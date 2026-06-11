//! Handler WASM (Tier 2) del módulo `pricing` — motor de pricing.
//! Portado de old_modules/m_pricing (PricingService.create_price_list /
//! get_price / calculate_discount) según `WASM-TODO.md` §1–§3 y
//! `architecture/modules/pricing.md`.
//!
//! Lógica pura, sin BD: recibe `{payload, context}` y devuelve **intenciones**
//! (ops SQL del mismo módulo) que el host valida y ejecuta en una transacción.
//! Todos los importes se calculan con `rust_decimal` (aritmética decimal
//! exacta, `quantize(0.01, HALF_UP)`) — nunca `f64` (WASM-TODO.md §5).
//!
//! Estado de los 3 exports declarados en `module.json`:
//!
//! * `create_price_list` — **operativo** con el ABI actual del host: valida
//!   payload/fechas y devuelve `pricing._unset_default` (si `is_default`) +
//!   `pricing._insert_price_list` en la MISMA transacción (invariante "una sola
//!   lista default por hub"). La unicidad `(hub_id, code)` la garantiza el
//!   índice único `uq_pricing_list_hub_code` (el host aún no entrega lecturas
//!   pre-cargadas para devolver `duplicate_code` amigable).
//! * `get_price` / `calculate_discount` — la lógica completa vive en
//!   [`get_price_compute`] / [`calculate_discount_compute`] (públicas, listas
//!   para cablear), pero el ABI actual del host (`erplora-wasm-host` +
//!   `runtime::commands::execute_wasm`) NO entrega lecturas pre-cargadas ni
//!   retorna el resultado de un handler read-only al caller (`Output` solo
//!   lleva operations+events y el command responde `{ok:true}`). Hasta que el
//!   humano defina ese contrato en el core, los exports devuelven un error
//!   explícito `unsupported_readonly_handler`.

use erplora_guest_sdk::{Operation, Output};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{json, Map, Value};
use std::str::FromStr;

#[cfg(feature = "guest")]
use extism_pdk::*;

/// Mensaje de los exports read-only mientras el host no soporte lecturas
/// pre-cargadas + retorno de resultado (decisión core pendiente).
pub const UNSUPPORTED_READONLY: &str = "unsupported_readonly_handler: el host aún no entrega \
lecturas pre-cargadas ni retorna el resultado de un handler read-only; la lógica está lista en \
pricing-handler::get_price_compute / calculate_discount_compute (ver WASM-TODO.md §2–§3)";

#[cfg(feature = "guest")]
#[plugin_fn]
pub fn create_price_list(input: Json<erplora_guest_sdk::Input>) -> FnResult<Json<Output>> {
    match create_price_list_pure(input.into_inner().into_value()) {
        Ok(out) => Ok(Json(out)),
        Err(code) => Err(WithReturnCode::new(Error::msg(code), 1)),
    }
}

#[cfg(feature = "guest")]
#[plugin_fn]
pub fn get_price(_input: Json<erplora_guest_sdk::Input>) -> FnResult<Json<Output>> {
    Err(WithReturnCode::new(Error::msg(UNSUPPORTED_READONLY.to_string()), 1))
}

#[cfg(feature = "guest")]
#[plugin_fn]
pub fn calculate_discount(_input: Json<erplora_guest_sdk::Input>) -> FnResult<Json<Output>> {
    Err(WithReturnCode::new(Error::msg(UNSUPPORTED_READONLY.to_string()), 1))
}

// ───────────────────────── helpers JSON/Decimal ─────────────────────────

fn as_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn as_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        Value::String(s) => matches!(s.as_str(), "1" | "true" | "True" | "yes"),
        _ => false,
    }
}

/// Parsea un JSON number/string a `Decimal` exacto (None si no es parseable).
fn dec(v: &Value) -> Option<Decimal> {
    match v {
        Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() { None } else { Decimal::from_str(t).ok() }
        }
        _ => None,
    }
}

/// Como [`dec`] pero tratando ausencia/null como None.
fn dec_opt(v: Option<&Value>) -> Option<Decimal> {
    match v {
        None | Some(Value::Null) => None,
        Some(x) => dec(x),
    }
}

/// `quantize(0.01, HALF_UP)` (WASM-TODO.md §5).
fn q2(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

/// Importe quantizado a 2 decimales como string exacto (p.ej. `"97.50"`).
fn money(d: Decimal) -> Value {
    let mut x = q2(d);
    x.rescale(2);
    json!(x.to_string())
}

/// String opcional: trim; vacío/null → None.
fn opt_str(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(x) => {
            let s = as_str(x).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
    }
}

/// Valida una fecha ISO `YYYY-MM-DD` (chequeo estructural, sin calendario).
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| s[r].chars().all(|c| c.is_ascii_digit());
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let m: u32 = s[5..7].parse().unwrap_or(0);
    let d: u32 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// `valid_from`/`valid_until`: vacío/null → NULL; mal formada → `invalid_date`.
fn parse_date_opt(v: Option<&Value>) -> Result<Value, String> {
    match opt_str(v) {
        None => Ok(Value::Null),
        Some(s) if is_iso_date(&s) => Ok(json!(s)),
        Some(s) => Err(format!("invalid_date: `{s}` no es una fecha ISO YYYY-MM-DD")),
    }
}

// ───────────────────── §1 create_price_list (operativo) ─────────────────────

/// Lógica pura de `pricing.price_lists.create` (WASM-TODO.md §1).
///
/// Valida `code`/`name`/fechas y, si `is_default`, antepone el subcomando
/// `pricing._unset_default` al INSERT (`pricing._insert_price_list`) para que
/// el flip del default previo y el alta corran en la MISMA transacción
/// (invariante "una sola lista default por hub").
pub fn create_price_list_pure(input: Value) -> Result<Output, String> {
    let payload = input.get("payload").cloned().unwrap_or(Value::Null);
    let context = input.get("context").cloned().unwrap_or(Value::Null);
    let empty: Vec<Value> = Vec::new();
    let new_ids = context.get("new_ids").and_then(|v| v.as_array()).unwrap_or(&empty);
    // El host es la única autoridad de ids: el guest toma el id de la lista de
    // `new_ids` y lo pasa explícito (`price_list_id`) para no depender del
    // `:new_id` que system_params regenera por operación.
    let id = new_ids.first().map(as_str).unwrap_or_default();
    if id.is_empty() {
        return Err("internal: context.new_ids vacío (host)".to_string());
    }

    let code = as_str(payload.get("code").unwrap_or(&Value::Null)).trim().to_string();
    if code.is_empty() {
        return Err("invalid_code: `code` no puede estar vacío".to_string());
    }
    let name = as_str(payload.get("name").unwrap_or(&Value::Null)).trim().to_string();
    if name.is_empty() {
        return Err("invalid_name: `name` no puede estar vacío".to_string());
    }
    let currency = opt_str(payload.get("currency")).unwrap_or_else(|| "EUR".to_string());
    let segment = opt_str(payload.get("segment"));
    let is_default = payload.get("is_default").map(as_bool).unwrap_or(false);
    let valid_from = parse_date_opt(payload.get("valid_from"))?;
    let valid_until = parse_date_opt(payload.get("valid_until"))?;

    let mut ops: Vec<Operation> = Vec::new();
    if is_default {
        // Invariante: baja la marca default previa ANTES del INSERT, misma tx.
        ops.push(Operation::sql("pricing._unset_default", Map::new()));
    }

    let mut p = Map::new();
    p.insert("price_list_id".into(), json!(id));
    p.insert("code".into(), json!(code));
    p.insert("name".into(), json!(name));
    p.insert("currency".into(), json!(currency));
    p.insert("is_default".into(), json!(is_default as i64));
    p.insert("valid_from".into(), valid_from);
    p.insert("valid_until".into(), valid_until);
    p.insert("segment".into(), segment.map(|s| json!(s)).unwrap_or(Value::Null));
    ops.push(Operation::sql("pricing._insert_price_list", p));

    // El evento `pricing.price_list.created` lo emite el host (declarado en el
    // `emit` del command en module.json); el handler no lo duplica.
    Ok(Output { operations: ops, events: vec![] })
}

// ─────────────── §2 get_price (lógica lista, pendiente de host) ───────────────

/// Lógica pura de `pricing.price_lists.get_price` (WASM-TODO.md §2, read-only).
///
/// * `payload` — `{product_ref, quantity?, price_list_id?, customer_segment?}`.
/// * `price_lists` — filas de las listas del hub (lectura pre-cargada por el
///   host: `pricing.price_lists.list`).
/// * `items` — filas de items del `product_ref` en esas listas (lectura
///   pre-cargada: `pricing.price_lists.items`).
///
/// Devuelve `{product_ref, price, quantity, price_list_id}` con el **menor**
/// precio entre los items cuyo bracket de cantidad matchea.
/// Errores: `invalid_quantity`, `invalid_id`, `no_price_list`, `no_price`.
pub fn get_price_compute(
    payload: &Value,
    price_lists: &[Value],
    items: &[Value],
) -> Result<Value, String> {
    let product_ref =
        as_str(payload.get("product_ref").unwrap_or(&Value::Null)).trim().to_string();
    if product_ref.is_empty() {
        return Err("invalid_payload: `product_ref` no puede estar vacío".to_string());
    }
    let quantity = match payload.get("quantity") {
        None | Some(Value::Null) => Decimal::ONE,
        Some(v) => dec(v).ok_or_else(|| "invalid_quantity".to_string())?,
    };
    if quantity <= Decimal::ZERO {
        return Err("invalid_quantity".to_string());
    }
    // `price_list_id` mal formado (no-string / vacío) → invalid_id.
    let req_list: Option<String> = match payload.get("price_list_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(_) => return Err("invalid_id".to_string()),
    };
    let customer_segment = opt_str(payload.get("customer_segment"));

    // Selección de listas candidatas (§2): por id explícito, o todas las
    // activas filtradas por segmento (== customer_segment o universal).
    let candidate_ids: Vec<String> = price_lists
        .iter()
        .filter(|l| {
            if !l.get("is_active").map(as_bool).unwrap_or(false) {
                return false;
            }
            if l.get("is_deleted").map(as_bool).unwrap_or(false) {
                return false;
            }
            if let Some(req) = &req_list {
                return as_str(l.get("id").unwrap_or(&Value::Null)) == *req;
            }
            if let Some(cs) = &customer_segment {
                let seg = opt_str(l.get("segment"));
                return seg.is_none() || seg.as_deref() == Some(cs.as_str());
            }
            true
        })
        .map(|l| as_str(l.get("id").unwrap_or(&Value::Null)))
        .collect();
    if candidate_ids.is_empty() {
        return Err("no_price_list".to_string());
    }

    // Matching por bracket de cantidad; mejor precio = el MENOR entre los matches.
    let mut best: Option<(Decimal, Value, String)> = None; // (price, raw_price, list_id)
    for it in items {
        if it.get("is_deleted").map(as_bool).unwrap_or(false) {
            continue;
        }
        if as_str(it.get("product_ref").unwrap_or(&Value::Null)) != product_ref {
            continue;
        }
        let list_id = as_str(it.get("price_list_id").unwrap_or(&Value::Null));
        if !candidate_ids.contains(&list_id) {
            continue;
        }
        let min_q = dec_opt(it.get("min_quantity")).unwrap_or(Decimal::ONE);
        if quantity < min_q {
            continue;
        }
        if let Some(max_q) = dec_opt(it.get("max_quantity")) {
            if quantity > max_q {
                continue;
            }
        }
        let raw_price = it.get("price").cloned().unwrap_or(Value::Null);
        let price = match dec(&raw_price) {
            Some(p) => p,
            None => continue,
        };
        if best.as_ref().map(|(b, _, _)| price < *b).unwrap_or(true) {
            best = Some((price, raw_price, list_id));
        }
    }

    match best {
        Some((_, raw_price, list_id)) => Ok(json!({
            "product_ref": product_ref,
            "price": raw_price,
            "quantity": quantity.normalize().to_string(),
            "price_list_id": list_id,
        })),
        None => Err("no_price".to_string()),
    }
}

// ─────────── §3 calculate_discount (lógica lista, pendiente de host) ───────────

/// Lógica pura de `pricing.rules.calculate_discount` (WASM-TODO.md §3, read-only).
///
/// * `payload` — `{amount, rules?, customer_segment?}`; `rules` admite null
///   (todas las activas), lista de ids, lista de dicts inline o modo mixto.
/// * `db_rules` — reglas activas del hub leídas por el host (lectura
///   pre-cargada: `pricing.rules.list`); las inline llegan en el payload.
///
/// Aplica las reglas por `priority` ascendente (desempate por `code`) de forma
/// iterativa sobre el running total, con clamp a `[0, running]` y quantize 0.01.
/// Devuelve `{original_amount, final_amount, total_discount, applied_rules}`.
/// Error: `invalid_amount`.
pub fn calculate_discount_compute(payload: &Value, db_rules: &[Value]) -> Result<Value, String> {
    let amount = dec_opt(payload.get("amount")).ok_or_else(|| "invalid_amount".to_string())?;
    if amount < Decimal::ZERO {
        return Err("invalid_amount".to_string());
    }
    let customer_segment = opt_str(payload.get("customer_segment"));

    // Resolución de reglas (null / ids / inline / mixto).
    let mut resolved: Vec<Value> = Vec::new();
    match payload.get("rules") {
        None | Some(Value::Null) => resolved = db_rules.to_vec(),
        Some(Value::Array(arr)) => {
            for el in arr {
                match el {
                    Value::String(id) => {
                        if let Some(r) = db_rules
                            .iter()
                            .find(|r| as_str(r.get("id").unwrap_or(&Value::Null)) == *id)
                        {
                            resolved.push(r.clone());
                        }
                    }
                    Value::Object(_) => resolved.push(el.clone()),
                    _ => {}
                }
            }
        }
        Some(_) => return Err("invalid_payload: `rules` debe ser null o lista".to_string()),
    }

    // Orden de aplicación: priority asc (menor antes), desempate por code.
    resolved.sort_by(|a, b| {
        let ka = (
            dec_opt(a.get("priority")).and_then(|d| d.to_i64()).unwrap_or(100),
            as_str(a.get("code").unwrap_or(&Value::Null)),
        );
        let kb = (
            dec_opt(b.get("priority")).and_then(|d| d.to_i64()).unwrap_or(100),
            as_str(b.get("code").unwrap_or(&Value::Null)),
        );
        ka.cmp(&kb)
    });

    let original = q2(amount);
    let mut running = original;
    let mut applied: Vec<Value> = Vec::new();
    let hundred = Decimal::from(100);

    for r in &resolved {
        // `conditions` viaja como JSON string desde la BD; inline puede ser objeto.
        let conditions: Value = match r.get("conditions") {
            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
            Some(v @ Value::Object(_)) => v.clone(),
            _ => json!({}),
        };
        // Filtros de aplicabilidad contra el running actual.
        if let Some(min) = dec_opt(r.get("min_amount")) {
            if running < min {
                continue;
            }
        }
        if let Some(max) = dec_opt(r.get("max_amount")) {
            if running > max {
                continue;
            }
        }
        if as_str(r.get("applies_to").unwrap_or(&Value::Null)) == "customer_segment" {
            let seg = opt_str(conditions.get("segment"));
            if seg.is_none() || seg.as_deref() != customer_segment.as_deref() {
                continue;
            }
        }

        let value = dec_opt(r.get("value")).unwrap_or(Decimal::ZERO);
        let rule_type = {
            let t = as_str(r.get("rule_type").unwrap_or(&Value::Null));
            if t.is_empty() { "percent".to_string() } else { t }
        };
        let delta = match rule_type.as_str() {
            "percent" => running * value / hundred,
            "fixed" => value,
            "tiered" => {
                // Mayor `discount` cuyo `min <= running`; sin tier que cualifique → saltar.
                let empty: Vec<Value> = Vec::new();
                let tiers = conditions.get("tiers").and_then(|v| v.as_array()).unwrap_or(&empty);
                let mut best: Option<Decimal> = None;
                for t in tiers {
                    let min = dec_opt(t.get("min")).unwrap_or(Decimal::ZERO);
                    if min <= running {
                        let disc = dec_opt(t.get("discount")).unwrap_or(Decimal::ZERO);
                        if best.map(|b| disc > b).unwrap_or(true) {
                            best = Some(disc);
                        }
                    }
                }
                match best {
                    None => continue,
                    Some(t) => {
                        let unit = opt_str(conditions.get("unit"))
                            .unwrap_or_else(|| "percent".to_string());
                        if unit == "fixed" { t } else { running * t / hundred }
                    }
                }
            }
            "buy_x_get_y" => {
                let buy = dec_opt(conditions.get("buy")).unwrap_or(Decimal::ZERO);
                let get = dec_opt(conditions.get("get")).unwrap_or(Decimal::ZERO);
                let unit_price = dec_opt(conditions.get("unit_price")).unwrap_or(Decimal::ZERO);
                let qty = dec_opt(conditions.get("quantity")).unwrap_or(Decimal::ZERO);
                let group = buy + get;
                if group <= Decimal::ZERO {
                    continue;
                }
                let sets = (qty / group).floor();
                unit_price * sets * get
            }
            _ => continue, // rule_type desconocido → saltar la regla.
        };

        // Clamp a [0, running] y quantize 0.01 HALF_UP.
        let delta = if delta < Decimal::ZERO {
            Decimal::ZERO
        } else if delta > running {
            running
        } else {
            delta
        };
        let delta_q = q2(delta);
        running = q2(running - delta_q);
        applied.push(json!({
            "code": as_str(r.get("code").unwrap_or(&Value::Null)),
            "rule_type": rule_type,
            "value": r.get("value").cloned().unwrap_or(Value::Null),
            "discount_amount": money(delta_q),
            "priority": r.get("priority").cloned().unwrap_or(json!(100)),
        }));
    }

    Ok(json!({
        "original_amount": money(original),
        "final_amount": money(running),
        "total_discount": money(original - running),
        "applied_rules": applied,
    }))
}
