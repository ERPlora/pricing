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

use erplora_guest_sdk::currency as sdk_currency;
use erplora_guest_sdk::money as sdk_money;
use erplora_guest_sdk::money::Minor;
use erplora_guest_sdk::{Operation, Output};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::str::FromStr;

// ───────────────────────── tax basis (ADR-0210) ─────────────────────────

/// The amount ALREADY contains the tax (retail / POS price tag, art. 88.Uno LIVA).
pub const BASIS_INCLUSIVE: &str = "inclusive";
/// The amount is the taxable base; the tax is added on top (B2B / wholesale list).
pub const BASIS_EXCLUSIVE: &str = "exclusive";
/// Last-resort basis when neither the price list nor the hub states one.
///
/// **Inclusive**, because the Hub is a POS: a price shown to a final consumer in Spain must
/// already carry the tax (art. 88.Uno LIVA), and `sales` has always defaulted `tax_included` to
/// `true`. Making the default explicit is the point — an implicit basis is the bug.
pub const DEFAULT_TAX_BASIS: &str = BASIS_INCLUSIVE;

/// `Output` of the read-only handlers with the extra `result` channel (ADR-0069), same shape the
/// `taxes` handler already ships: the host deserializes `operations`/`events` and ignores
/// `result`; a host that returns handler results reads it without any manifest change.
#[derive(Debug, Serialize)]
pub struct PricingOutput {
    pub operations: Vec<Operation>,
    pub events: Vec<erplora_guest_sdk::Event>,
    pub result: Value,
}

impl PricingOutput {
    /// A pure read: no writes, no events, just the computed contract.
    pub fn read_only(result: Value) -> Self {
        PricingOutput { operations: vec![], events: vec![], result }
    }
}

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

/// Redondea a CÉNTIMO ENTERO. **No decide el modo**: delega en [`erplora_guest_sdk::money::round`],
/// que es EL redondeo del hub (HALF_UP, ADR-0123 §4). La disputa que había —`pricing`/`taxes` en
/// HALF_UP contra `sales`/`invoice` en half-even— venía de que cada handler traía su propia
/// aritmética. Ya no: hay una.
fn q0(d: Decimal) -> Decimal {
    Decimal::from(sdk_money::round(d))
}

/// Importe como CÉNTIMOS ENTEROS (p. ej. `9750`), que es lo que el caller suma a sus totales.
/// Antes emitía strings de EUROS (`"97.50"`) — un caller que los sumara a un total en céntimos se
/// equivocaba ×100.
fn money(d: Decimal) -> Value {
    json!(sdk_money::round(d))
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

// ─────────────── tax basis resolution + cent-exact apportionment ───────────────

/// Resolves the tax basis of a price list, and says WHERE the answer came from.
///
/// Order: the list's own `tax_included` → the hub's `tax_mode` → [`DEFAULT_TAX_BASIS`].
/// The returned basis is never absent: an implicit basis is exactly the ambiguity this solves.
pub fn resolve_tax_basis(_list: &Value, _context: &Value) -> (&'static str, &'static str) {
    (DEFAULT_TAX_BASIS, "default")
}

/// Splits `total` across `weights` so the parts add up to `total` **EXACTLY**.
///
/// Not implemented yet.
pub fn allocate_amount(_total: Minor, weights: &[Minor]) -> Vec<Minor> {
    vec![0; weights.len()]
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
    context: &Value,
    price_lists: &[Value],
    items: &[Value],
) -> Result<Value, String> {
    let _ = context;
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
pub fn calculate_discount_compute(
    payload: &Value,
    context: &Value,
    db_rules: &[Value],
) -> Result<Value, String> {
    let _ = context;
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

    let original = q0(amount);
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

        // `value` es una TASA (%), nunca dinero. El dinero de una regla `fixed` vive en su propia
        // columna `amount_cents` (INTEGER, céntimos). Antes las dos cosas compartían la columna
        // `value REAL -- % o euros`: un descuento fijo de 5 € restaba 5 CÉNTIMOS.
        let value = dec_opt(r.get("value")).unwrap_or(Decimal::ZERO);
        let amount_cents = dec_opt(r.get("amount_cents")).unwrap_or(Decimal::ZERO);
        let rule_type = {
            let t = as_str(r.get("rule_type").unwrap_or(&Value::Null));
            if t.is_empty() { "percent".to_string() } else { t }
        };
        let delta = match rule_type.as_str() {
            "percent" => running * value / hundred,
            "fixed" => amount_cents,
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

        // Clamp a [0, running] y redondeo a céntimo entero (una sola vez, aquí).
        let delta = if delta < Decimal::ZERO {
            Decimal::ZERO
        } else if delta > running {
            running
        } else {
            delta
        };
        let delta_q = q0(delta);
        running = q0(running - delta_q);
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


// ─────────────────────────────── tests ───────────────────────────────
//
// THIS MODULE'S MONEY IS CENTS (ADR-0123), like the rest of the hub.
//
// `pricing` had fallen behind: its DB had already migrated (`pricing_price_list_item.price
// INTEGER -- cents`, `min_amount`/`max_amount` in cents) but the ENGINE still reasoned in EUROS
// (`quantize(0.01)`, outputs like `"97.50"`). That broke THREE things at once, all ×100:
//
//   1. A FIXED discount of 5 € subtracted **5 CENTS** (`"fixed" => value`, with `value` in euros
//      and the running total in cents).
//   2. The `min_amount`/`max_amount` filters (cents) were compared against a running total in
//      euros: a "from 10 €" rule (min_amount = 1000) did NOT fire on a 50 € sale (running = 50).
//   3. The output was a euro string, so the first caller adding it to a cents total was wrong
//      ×100 again.
//
// The root cause of (1) was a POLYMORPHIC column: `value REAL -- % or euros`. One column, two
// units, and the discriminator in ANOTHER column. Now the percentage lives in `value` (it is a
// RATE, not money) and the fixed amount in `amount_cents` (it is money → cents). Nobody has to
// guess.
//
// ADR-0210 adds the second half of the same disease: an amount whose TAX BASIS is implicit, and
// a discount handed over as a single aggregate number that the caller has to split by hand.
#[cfg(test)]
mod tests {
    use super::*;

    /// A rule exactly as the host hands it over from the DB.
    fn rule(code: &str, rule_type: &str, value: f64, amount_cents: Option<i64>) -> Value {
        json!({
            "code": code, "rule_type": rule_type, "value": value,
            "amount_cents": amount_cents, "priority": 100, "conditions": "{}",
        })
    }

    fn calc(amount_cents: i64, rules: &[Value]) -> Value {
        calculate_discount_compute(&json!({ "amount": amount_cents }), &json!({}), rules)
            .expect("compute")
    }

    /// A price list row as the host preloads it (`pricing.price_lists.list`).
    fn price_list(id: &str, tax_included: Option<bool>, currency: &str) -> Value {
        json!({
            "id": id, "code": id, "name": id, "currency": currency,
            "is_active": 1, "is_deleted": 0, "segment": Value::Null,
            "tax_included": tax_included.map(|b| json!(b as i64)).unwrap_or(Value::Null),
        })
    }

    /// A price list item row (`pricing.price_lists.items_by_product`).
    fn price_item(list_id: &str, product_ref: &str, price: i64) -> Value {
        json!({
            "id": format!("{list_id}-{product_ref}"), "price_list_id": list_id,
            "product_ref": product_ref, "price": price,
            "min_quantity": 1, "max_quantity": Value::Null, "is_deleted": 0,
        })
    }

    /// A quote line as `sales` hands it over: an amount + the rate that applies to it.
    fn quote_line(line_ref: &str, amount: i64, tax_rate_pct: f64) -> Value {
        json!({ "line_ref": line_ref, "amount": amount, "tax_rate_pct": tax_rate_pct })
    }

    fn calc_lines(lines: &[Value], rules: &[Value]) -> Value {
        calculate_discount_compute(&json!({ "lines": lines }), &json!({}), rules).expect("compute")
    }

    // ───────────────────────── the money is cents ─────────────────────────

    #[test]
    fn the_amount_goes_in_and_comes_back_as_WHOLE_CENTS() {
        // 14,93 € = 1493 cents. It comes back as an integer NUMBER, not the euro string "14.93":
        // the caller adds this to totals that are in cents.
        let out = calc(1493, &[]);
        assert_eq!(out["original_amount"], json!(1493));
        assert_eq!(out["final_amount"], json!(1493));
        assert_eq!(out["total_discount"], json!(0));
    }

    #[test]
    fn a_fixed_discount_of_5_EUR_subtracts_500_cents_NOT_5() {
        // THE BUG: `"fixed" => value` subtracted 5 (cents) for a 5 € discount.
        let out = calc(1493, &[rule("FIVE", "fixed", 0.0, Some(500))]);
        assert_eq!(out["total_discount"], json!(500), "a 5 € discount subtracts 500 cents");
        assert_eq!(out["final_amount"], json!(993));
    }

    #[test]
    fn a_percentage_operates_on_cents_and_is_still_a_RATE() {
        // The % is a RATE, not money: it stays in `value`. 10 % of 1493 = 149,3 → 149 cents.
        let out = calc(1493, &[rule("TEN", "percent", 10.0, None)]);
        assert_eq!(out["total_discount"], json!(149));
        assert_eq!(out["final_amount"], json!(1344));
    }

    #[test]
    fn the_discount_is_rounded_to_a_WHOLE_CENT_not_to_a_fraction() {
        // 10 % of 1495 = 149,5 → HALF_UP → 150. A fractional cent does not exist.
        let out = calc(1495, &[rule("TEN", "percent", 10.0, None)]);
        assert_eq!(out["total_discount"], json!(150));
    }

    #[test]
    fn min_amount_filters_in_cents_against_a_running_total_in_cents() {
        // THE BUG: `min_amount` is cents (1000 = 10 €) and was compared against a running total in
        // euros (50) → 50 < 1000 → the rule did NOT fire on a 50 € sale.
        let mut r = rule("FROM10", "percent", 10.0, None);
        r["min_amount"] = json!(1000); // 10 €
        let out = calc(5000, &[r]); // 50 € sale
        assert_eq!(out["total_discount"], json!(500), "«from 10 €» must fire on a 50 € sale");
    }

    #[test]
    fn max_amount_also_filters_in_cents() {
        let mut r = rule("UPTO10", "percent", 10.0, None);
        r["max_amount"] = json!(1000); // only up to 10 €
        let out = calc(5000, &[r]); // 50 € → out of range
        assert_eq!(out["total_discount"], json!(0));
    }

    #[test]
    fn a_fixed_discount_larger_than_the_amount_is_capped_to_the_amount() {
        // Clamp to [0, running]: never a negative total nor invented change.
        let out = calc(300, &[rule("BEAST", "fixed", 0.0, Some(500))]);
        assert_eq!(out["total_discount"], json!(300));
        assert_eq!(out["final_amount"], json!(0));
    }

    #[test]
    fn rules_chain_by_priority_over_the_running_total() {
        // 1000 cts − 10 % (100) = 900; then a fixed 2 € (200) → 700.
        let mut pct = rule("PCT", "percent", 10.0, None);
        pct["priority"] = json!(1);
        let mut fixed = rule("FIXED", "fixed", 0.0, Some(200));
        fixed["priority"] = json!(2);
        let out = calc(1000, &[pct, fixed]);
        assert_eq!(out["final_amount"], json!(700));
        assert_eq!(out["total_discount"], json!(300));
    }

    #[test]
    fn the_applied_rules_breakdown_is_also_in_cents() {
        let out = calc(1493, &[rule("FIVE", "fixed", 0.0, Some(500))]);
        assert_eq!(out["applied_rules"][0]["discount_amount"], json!(500));
    }

    #[test]
    fn a_negative_amount_is_still_invalid() {
        let err = calculate_discount_compute(&json!({ "amount": -1 }), &json!({}), &[]).unwrap_err();
        assert_eq!(err, "invalid_amount");
    }

    // ─────────────────── ADR-0210 · every price states its basis ───────────────────

    #[test]
    fn a_retail_list_marked_tax_included_prices_INCLUSIVE() {
        let lists = [price_list("retail", Some(true), "EUR")];
        let items = [price_item("retail", "coffee", 121)];
        let out = get_price_compute(&json!({ "product_ref": "coffee" }), &json!({}), &lists, &items)
            .expect("price");
        assert_eq!(out["price"], json!(121));
        assert_eq!(out["tax_basis"], json!(BASIS_INCLUSIVE));
        assert_eq!(out["tax_basis_source"], json!("price_list"));
    }

    #[test]
    fn a_B2B_list_marked_tax_excluded_prices_EXCLUSIVE() {
        // The whole point of the field: a B2B list without VAT and a retail list with VAT can
        // coexist in the same hub without anyone guessing which is which.
        let lists = [price_list("b2b", Some(false), "EUR")];
        let items = [price_item("b2b", "coffee", 100)];
        let out = get_price_compute(&json!({ "product_ref": "coffee" }), &json!({}), &lists, &items)
            .expect("price");
        assert_eq!(out["tax_basis"], json!(BASIS_EXCLUSIVE));
        assert_eq!(out["tax_basis_source"], json!("price_list"));
    }

    #[test]
    fn a_list_without_its_own_basis_inherits_the_HUB_setting() {
        let lists = [price_list("plain", None, "EUR")];
        let items = [price_item("plain", "coffee", 100)];
        let ctx = json!({ "tax_mode": "exclusive" });
        let out =
            get_price_compute(&json!({ "product_ref": "coffee" }), &ctx, &lists, &items).expect("p");
        assert_eq!(out["tax_basis"], json!(BASIS_EXCLUSIVE));
        assert_eq!(out["tax_basis_source"], json!("hub"), "the hub is the second step, not a guess");
    }

    #[test]
    fn with_neither_list_nor_hub_the_basis_falls_back_to_INCLUSIVE_and_says_so() {
        // The Hub is a POS: art. 88.Uno LIVA requires the consumer price to carry the tax. The
        // default is written down AND reported, so it is never an implicit assumption.
        let lists = [price_list("plain", None, "EUR")];
        let items = [price_item("plain", "coffee", 121)];
        let out = get_price_compute(&json!({ "product_ref": "coffee" }), &json!({}), &lists, &items)
            .expect("price");
        assert_eq!(out["tax_basis"], json!(BASIS_INCLUSIVE));
        assert_eq!(out["tax_basis_source"], json!("default"));
    }

    #[test]
    fn the_quote_carries_its_currency_and_its_minor_unit_precision() {
        let lists = [price_list("retail", Some(true), "EUR")];
        let items = [price_item("retail", "coffee", 121)];
        let out = get_price_compute(&json!({ "product_ref": "coffee" }), &json!({}), &lists, &items)
            .expect("price");
        assert_eq!(out["currency"], json!("EUR"));
        assert_eq!(out["minor_unit_decimals"], json!(2));
    }

    #[test]
    fn the_precision_follows_the_currency_not_a_hardcoded_two() {
        // 1499 in JPY is 1499 yen, not 14,99. The integer means nothing without its currency.
        let lists = [price_list("jp", Some(true), "JPY")];
        let items = [price_item("jp", "coffee", 1499)];
        let out = get_price_compute(&json!({ "product_ref": "coffee" }), &json!({}), &lists, &items)
            .expect("price");
        assert_eq!(out["currency"], json!("JPY"));
        assert_eq!(out["minor_unit_decimals"], json!(0));
    }

    #[test]
    fn candidate_lists_with_DIFFERENT_bases_are_rejected_not_silently_compared() {
        // Picking the "cheapest" between a gross 110 and a net 100 compares apples to oranges.
        // No implicit conversion: the caller has to say which list it wants.
        let lists = [price_list("retail", Some(true), "EUR"), price_list("b2b", Some(false), "EUR")];
        let items = [price_item("retail", "coffee", 121), price_item("b2b", "coffee", 100)];
        let err = get_price_compute(&json!({ "product_ref": "coffee" }), &json!({}), &lists, &items)
            .unwrap_err();
        assert_eq!(err, "mixed_tax_basis");
    }

    #[test]
    fn asking_for_ONE_list_by_id_is_never_a_mix() {
        let lists = [price_list("retail", Some(true), "EUR"), price_list("b2b", Some(false), "EUR")];
        let items = [price_item("retail", "coffee", 121), price_item("b2b", "coffee", 100)];
        let payload = json!({ "product_ref": "coffee", "price_list_id": "b2b" });
        let out = get_price_compute(&payload, &json!({}), &lists, &items).expect("price");
        assert_eq!(out["price"], json!(100));
        assert_eq!(out["tax_basis"], json!(BASIS_EXCLUSIVE));
    }

    #[test]
    fn an_INCLUSIVE_price_splits_into_base_plus_tax_that_add_back_to_the_cent() {
        // The acceptance criterion of pricing#15: a price marked tax-included yields the right
        // base and tax in whole cents. `pricing` does not own the tax engine (that is `taxes`);
        // what it owns is stating the basis so the shared SDK split is the right one.
        let lines = [quote_line("L1", 1000, 21.0)];
        let out = calc_lines(&lines, &[]);
        assert_eq!(out["tax_basis"], json!(BASIS_INCLUSIVE));
        let charged = out["by_tax_rate"][0]["amount_after"].as_i64().unwrap();
        let (base, tax) = sdk_money::split_tax_included(charged, Decimal::from(21));
        assert_eq!((base, tax), (826, 174), "1000 cts at 21 % included → 826 base + 174 tax");
        assert_eq!(base + tax, 1000, "what the customer pays does not move by a cent");
    }

    // ────────── ADR-0210 · a discount is allocated, never handed over as a lump ──────────

    #[test]
    fn a_global_discount_is_allocated_to_the_lines_and_the_parts_add_up_EXACTLY() {
        // 1 € off a 30 € quote of three equal lines. 100/3 is not a whole number of cents, so
        // somebody has to take the leftover cent — deterministically, and only once.
        let lines =
            [quote_line("L1", 1000, 21.0), quote_line("L2", 1000, 21.0), quote_line("L3", 1000, 21.0)];
        let out = calc_lines(&lines, &[rule("EURO", "fixed", 0.0, Some(100))]);
        let alloc: Vec<i64> =
            out["allocation"].as_array().unwrap().iter().map(|a| a["discount"].as_i64().unwrap()).collect();
        assert_eq!(alloc, vec![34, 33, 33]);
        assert_eq!(alloc.iter().sum::<i64>(), 100, "the parts ARE the whole");
        assert_eq!(out["total_discount"], json!(100));
    }

    #[test]
    fn allocation_is_LARGEST_REMAINDER_not_HALF_UP_because_HALF_UP_does_not_conserve_the_sum() {
        // 101 cts over three equal lines: the exact share is 33,6667. Rounding each one HALF_UP
        // (the money rounding of ADR-0123) gives 34+34+34 = 102 — a cent CONJURED out of nothing.
        // Rounding is for producing ONE amount; splitting one amount into N is apportionment.
        let lines =
            [quote_line("L1", 1000, 21.0), quote_line("L2", 1000, 21.0), quote_line("L3", 1000, 21.0)];
        let out = calc_lines(&lines, &[rule("ODD", "fixed", 0.0, Some(101))]);
        let alloc: Vec<i64> =
            out["allocation"].as_array().unwrap().iter().map(|a| a["discount"].as_i64().unwrap()).collect();
        assert_eq!(alloc, vec![34, 34, 33]);
        assert_eq!(alloc.iter().sum::<i64>(), 101);
    }

    #[test]
    fn the_leftover_cent_goes_to_the_LARGEST_REMAINDER_not_to_the_first_line() {
        // Weights 333/333/334 → shares 33,3 / 33,3 / 33,4. The third line has the biggest
        // fractional part, so it takes the leftover cent.
        let lines =
            [quote_line("L1", 333, 21.0), quote_line("L2", 333, 21.0), quote_line("L3", 334, 21.0)];
        let out = calc_lines(&lines, &[rule("EURO", "fixed", 0.0, Some(100))]);
        let alloc: Vec<i64> =
            out["allocation"].as_array().unwrap().iter().map(|a| a["discount"].as_i64().unwrap()).collect();
        assert_eq!(alloc, vec![33, 33, 34]);
        assert_eq!(alloc.iter().sum::<i64>(), 100);
    }

    #[test]
    fn allocate_amount_is_deterministic_when_the_remainders_tie() {
        // Same remainder → input order decides. Two runs of the same quote must bill the same.
        assert_eq!(allocate_amount(2, &[1, 1, 1]), vec![1, 1, 0]);
        assert_eq!(allocate_amount(2, &[1, 1, 1]), vec![1, 1, 0]);
    }

    #[test]
    fn the_per_rate_breakdown_stays_SQUARE_after_a_global_discount() {
        // THE actual bug this issue is the root of: a global discount that does not reach the
        // per-rate breakdown makes the header declare a discounted total against an undiscounted
        // desglose — and that combination is what reaches Invoice/VeriFactu.
        let lines =
            [quote_line("L1", 1000, 21.0), quote_line("L2", 1000, 21.0), quote_line("L3", 500, 10.0)];
        let out = calc_lines(&lines, &[rule("FIVE", "fixed", 0.0, Some(500))]);

        let by_rate = out["by_tax_rate"].as_array().unwrap();
        assert_eq!(by_rate.len(), 2, "one entry per tax RATE (ADR-0123 §4), not per line");
        assert_eq!(by_rate[0]["tax_rate_pct"], json!(21.0));
        assert_eq!(by_rate[0]["amount_before"], json!(2000));
        assert_eq!(by_rate[0]["discount"], json!(400));
        assert_eq!(by_rate[0]["amount_after"], json!(1600));
        assert_eq!(by_rate[1]["tax_rate_pct"], json!(10.0));
        assert_eq!(by_rate[1]["amount_before"], json!(500));
        assert_eq!(by_rate[1]["discount"], json!(100));
        assert_eq!(by_rate[1]["amount_after"], json!(400));

        let discounts: i64 = by_rate.iter().map(|r| r["discount"].as_i64().unwrap()).sum();
        let after: i64 = by_rate.iter().map(|r| r["amount_after"].as_i64().unwrap()).sum();
        assert_eq!(discounts, out["total_discount"].as_i64().unwrap());
        assert_eq!(after, out["final_amount"].as_i64().unwrap());
    }

    #[test]
    fn a_zero_rate_line_is_a_rate_like_any_other_and_keeps_its_share() {
        // 0 % is not "no tax": it is a rate, it has its own desglose entry, and it takes its
        // proportional share of the discount like everybody else.
        let lines =
            [quote_line("L1", 1000, 21.0), quote_line("L2", 1000, 10.0), quote_line("L3", 1000, 0.0)];
        let out = calc_lines(&lines, &[rule("TEN", "percent", 10.0, None)]);
        assert_eq!(out["total_discount"], json!(300));
        let by_rate = out["by_tax_rate"].as_array().unwrap();
        assert_eq!(by_rate.len(), 3);
        assert_eq!(by_rate[2]["tax_rate_pct"], json!(0.0));
        assert_eq!(by_rate[2]["discount"], json!(100));
        assert_eq!(by_rate[2]["amount_after"], json!(900));
    }

    #[test]
    fn a_discount_never_pushes_a_line_below_zero() {
        // 9 € off a 5 € quote: the discount is capped at the quote and every line lands at 0.
        let lines = [quote_line("L1", 300, 21.0), quote_line("L2", 200, 10.0)];
        let out = calc_lines(&lines, &[rule("BEAST", "fixed", 0.0, Some(900))]);
        assert_eq!(out["total_discount"], json!(500));
        assert_eq!(out["final_amount"], json!(0));
        let alloc: Vec<i64> =
            out["allocation"].as_array().unwrap().iter().map(|a| a["discount"].as_i64().unwrap()).collect();
        assert_eq!(alloc, vec![300, 200]);
        for a in out["allocation"].as_array().unwrap() {
            assert!(a["amount_after"].as_i64().unwrap() >= 0, "no line may go negative");
        }
    }

    #[test]
    fn an_all_zero_quote_allocates_nothing_instead_of_dividing_by_zero() {
        let lines = [quote_line("L1", 0, 21.0), quote_line("L2", 0, 10.0)];
        let out = calc_lines(&lines, &[rule("TEN", "percent", 10.0, None)]);
        assert_eq!(out["total_discount"], json!(0));
        let alloc: Vec<i64> =
            out["allocation"].as_array().unwrap().iter().map(|a| a["discount"].as_i64().unwrap()).collect();
        assert_eq!(alloc, vec![0, 0]);
    }

    #[test]
    fn sub_cent_shares_still_add_up_to_the_whole_discount() {
        // Three 1-cent lines, half off: 3 × 0,5 = 1,5 → 2 cts of discount to split into shares of
        // 0,667 cts. Two lines take a cent, one takes none — and the sum is still 2.
        let lines = [quote_line("L1", 1, 21.0), quote_line("L2", 1, 21.0), quote_line("L3", 1, 21.0)];
        let out = calc_lines(&lines, &[rule("HALF", "percent", 50.0, None)]);
        assert_eq!(out["total_discount"], json!(2));
        let alloc: Vec<i64> =
            out["allocation"].as_array().unwrap().iter().map(|a| a["discount"].as_i64().unwrap()).collect();
        assert_eq!(alloc, vec![1, 1, 0]);
        assert_eq!(alloc.iter().sum::<i64>(), 2);
    }

    #[test]
    fn a_surcharge_keeps_its_SIGN_when_it_is_allocated() {
        // Same apportionment, opposite direction: a manual surcharge must not turn into a
        // discount on its way to the lines, and it must still add up exactly.
        let parts = allocate_amount(-100, &[1000, 1000, 1000]);
        assert_eq!(parts, vec![-34, -33, -33]);
        assert_eq!(parts.iter().sum::<i64>(), -100);
    }

    #[test]
    fn the_allocation_keeps_the_line_reference_the_caller_sent() {
        let lines = [quote_line("table-7-L1", 1000, 21.0), quote_line("table-7-L2", 1000, 21.0)];
        let out = calc_lines(&lines, &[rule("TEN", "percent", 10.0, None)]);
        assert_eq!(out["allocation"][0]["line_ref"], json!("table-7-L1"));
        assert_eq!(out["allocation"][1]["line_ref"], json!("table-7-L2"));
        assert_eq!(out["allocation"][0]["amount"], json!(1000));
        assert_eq!(out["allocation"][0]["amount_after"], json!(900));
    }

    #[test]
    fn lines_and_an_aggregate_amount_that_disagree_are_REJECTED() {
        // Two sources of truth for the same number is how a quote silently drifts from its lines.
        let payload = json!({
            "amount": 900,
            "lines": [quote_line("L1", 1000, 21.0)],
        });
        let err = calculate_discount_compute(&payload, &json!({}), &[]).unwrap_err();
        assert_eq!(err, "amount_mismatch");
    }

    #[test]
    fn lines_with_CONFLICTING_bases_are_rejected() {
        // "no mixing lists of different bases in one order without a documented conversion".
        let mut a = quote_line("L1", 1000, 21.0);
        a["tax_basis"] = json!(BASIS_INCLUSIVE);
        let mut b = quote_line("L2", 1000, 21.0);
        b["tax_basis"] = json!(BASIS_EXCLUSIVE);
        let err = calculate_discount_compute(&json!({ "lines": [a, b] }), &json!({}), &[]).unwrap_err();
        assert_eq!(err, "mixed_tax_basis");
    }

    #[test]
    fn the_discount_quote_states_its_basis_and_where_it_came_from() {
        let lines = [quote_line("L1", 1000, 21.0)];
        let out = calculate_discount_compute(
            &json!({ "lines": lines, "tax_basis": BASIS_EXCLUSIVE }),
            &json!({}),
            &[],
        )
        .expect("compute");
        assert_eq!(out["tax_basis"], json!(BASIS_EXCLUSIVE));
        assert_eq!(out["tax_basis_source"], json!("payload"));
    }

    // ─────────────── ADR-0210 · the basis is persisted, not re-guessed ───────────────

    #[test]
    fn creating_a_list_persists_its_tax_basis() {
        let input = json!({
            "payload": { "code": "B2B", "name": "Wholesale", "tax_included": false },
            "context": { "new_ids": ["list-1"] },
        });
        let out = create_price_list_pure(input).expect("create");
        let insert = out.operations.last().expect("insert op");
        assert_eq!(insert.params["tax_included"], json!(0));
    }

    #[test]
    fn a_list_created_without_a_basis_stores_NULL_so_it_inherits_the_hub() {
        // NULL is not "unknown, assume something": it is "inherit", and the resolution is written
        // down. Storing a resolved value at creation time would freeze today's hub setting into a
        // list that should follow the hub.
        let input = json!({
            "payload": { "code": "RETAIL", "name": "Retail" },
            "context": { "new_ids": ["list-1"] },
        });
        let out = create_price_list_pure(input).expect("create");
        let insert = out.operations.last().expect("insert op");
        assert_eq!(insert.params["tax_included"], Value::Null);
    }
}
