//! WASM handler (Tier 2) of the `pricing` module — the pricing engine.
//!
//! Pure logic, no DB: it receives `{payload, context}` and returns **intentions** (SQL ops of this
//! same module) that the host validates and runs in one transaction. Every amount is computed with
//! `rust_decimal` (exact decimal arithmetic) or in integers — **never `f64`**; the money and its
//! single HALF_UP rounding come from `erplora_guest_sdk::money` (ADR-0123), not from here.
//!
//! # A price is not an integer (ADR-0210)
//!
//! The same `121` is a gross price on a retail list and a net price on a B2B one, and nothing in
//! the number says which. So every quote this handler produces carries its **tax basis** (and
//! where that basis came from), its **currency** and that currency's **precision** — and lists of
//! different bases are never silently compared.
//!
//! And a discount is never handed over as a lump: with `lines`, `calculate_discount` returns the
//! **allocation** per line (largest remainder — the parts add up to the whole exactly) plus the
//! same thing rolled up **per tax rate**, which is the level the desglose lives at. An aggregate
//! discount that never reaches the rates is what lets a header declare a discounted total against
//! an undiscounted desglose (the root of `sales#23`).
//!
//! # The three exports
//!
//! * `create_price_list` — validates payload/dates and returns `pricing._unset_default` (when
//!   `is_default`) + `pricing._insert_price_list` in the SAME transaction (invariant: one default
//!   list per hub). `(hub_id, code)` uniqueness is enforced by the `uq_pricing_list_hub_code`
//!   unique index.
//! * `get_price` / `calculate_discount` — read-only. They take their rows from the host's
//!   preloaded `context.reads` (ADR-0069) and answer through the extra `result` channel of
//!   [`PricingOutput`], the same shape the `taxes` handler already ships: today's host
//!   deserializes `operations`/`events` and ignores `result`, and a host that returns handler
//!   results picks it up with no manifest change.

use erplora_guest_sdk::currency as sdk_currency;
use erplora_guest_sdk::money as sdk_money;
use erplora_guest_sdk::money::Minor;
// ADR-0147 §2.1: las cantidades se persisten y viajan como ENTERO de punto fijo, escala 10⁶.
use erplora_guest_sdk::units::QUANTITY_SCALE;
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
pub fn get_price(input: Json<erplora_guest_sdk::Input>) -> FnResult<Json<PricingOutput>> {
    let v = input.into_inner().into_value();
    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
    let context = v.get("context").cloned().unwrap_or(Value::Null);
    let lists = preloaded_rows(&context, "pricing.price_lists.list");
    let items = preloaded_rows(&context, "pricing.price_lists.items_by_product");
    match get_price_compute(&payload, &context, &lists, &items) {
        Ok(result) => Ok(Json(PricingOutput::read_only(result))),
        Err(code) => Err(WithReturnCode::new(Error::msg(code), 1)),
    }
}

#[cfg(feature = "guest")]
#[plugin_fn]
pub fn calculate_discount(input: Json<erplora_guest_sdk::Input>) -> FnResult<Json<PricingOutput>> {
    let v = input.into_inner().into_value();
    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
    let context = v.get("context").cloned().unwrap_or(Value::Null);
    let rules = preloaded_rows(&context, "pricing.rules.list");
    match calculate_discount_compute(&payload, &context, &rules) {
        Ok(result) => Ok(Json(PricingOutput::read_only(result))),
        Err(code) => Err(WithReturnCode::new(Error::msg(code), 1)),
    }
}

/// Rows the host preloaded for a declared `reads` query (ADR-0069), landed in
/// `context.reads["<query>"]`. Empty when the host did not preload — a read-only quote degrades to
/// "no rows" instead of trusting whatever the client claims.
pub fn preloaded_rows(context: &Value, query: &str) -> Vec<Value> {
    context
        .get("reads")
        .and_then(|r| r.get(query))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
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

/// Una CANTIDAD, en µ (punto fijo entero, escala 10⁶ — ADR-0147 §2.1).
///
/// Es deliberadamente estricto: acepta un entero JSON o la cadena de un entero, y **rechaza**
/// cualquier cosa con decimales. `0.5` no es «medio µ», es un llamante que no habla el contrato, y
/// truncarlo es exactamente cómo `as_i64` convertía 0,5 kg en 0 — el defecto que dio origen a
/// ADR-0147. Un rechazo se ve; un truncado se cobra.
fn micro(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Como [`micro`] pero tratando ausencia/null como None.
fn micro_opt(v: Option<&Value>) -> Option<i64> {
    match v {
        None | Some(Value::Null) => None,
        Some(x) => micro(x),
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

/// `tax_included` as stored on the price list: `1`/`0`, or absent/NULL meaning "inherit".
///
/// NULL is **not** "unknown, assume something": it is "follow the hub". Freezing today's hub
/// setting into the row at creation time would silently detach the list from the hub later.
fn basis_from_flag(v: Option<&Value>) -> Option<&'static str> {
    match v {
        None | Some(Value::Null) => None,
        Some(x) => Some(if as_bool(x) { BASIS_INCLUSIVE } else { BASIS_EXCLUSIVE }),
    }
}

/// A basis spelled out as a string (`hub_settings.tax_mode`, or a caller's `tax_basis`).
/// Anything else is treated as **unset**, never as a guess.
fn basis_from_mode(v: Option<&Value>) -> Option<&'static str> {
    match opt_str(v)?.to_ascii_lowercase().as_str() {
        BASIS_INCLUSIVE => Some(BASIS_INCLUSIVE),
        BASIS_EXCLUSIVE => Some(BASIS_EXCLUSIVE),
        _ => None,
    }
}

/// Resolves the tax basis of a price list, and says WHERE the answer came from.
///
/// Order: the list's own `tax_included` → the hub's `tax_mode` → [`DEFAULT_TAX_BASIS`].
/// The returned basis is never absent, and its `source` is part of the answer: an implicit basis
/// is exactly the ambiguity this resolves, so "inclusive because nobody said otherwise" and
/// "inclusive because this list says so" must not look the same to the caller.
pub fn resolve_tax_basis(list: &Value, context: &Value) -> (&'static str, &'static str) {
    if let Some(b) = basis_from_flag(list.get("tax_included")) {
        return (b, "price_list");
    }
    if let Some(b) = basis_from_mode(context.get("tax_mode")) {
        return (b, "hub");
    }
    (DEFAULT_TAX_BASIS, "default")
}

/// Splits `total` across `weights` so the parts add up to `total` **EXACTLY**.
///
/// # Why this is NOT [`sdk_money::round`]
///
/// Rounding (HALF_UP, ADR-0123) produces **one** amount from a fraction. Apportionment splits
/// **one** amount into N without creating or destroying a cent, and rounding each share
/// independently does not do that: 101 cts over three equal lines is 33,6667 each, and HALF_UP
/// gives 34+34+34 = **102**. A cent out of nowhere is a desglose that no longer squares.
///
/// So the method is **largest remainder** (Hamilton): every line gets the floor of its exact
/// share, and the leftover cents go one each to the lines with the biggest fractional part, ties
/// broken by input order. Deterministic, sum-exact, and no line drifts more than one cent from
/// its proportional share.
///
/// All of it in integers (`i128` intermediate): the shares are never materialised as fractions,
/// so there is nothing to round in the first place.
///
/// The **sign travels**: a surcharge allocated is still a surcharge on every line. A discount is
/// capped at the sum of the weights (nothing goes below zero); a surcharge is not capped.
pub fn allocate_amount(total: Minor, weights: &[Minor]) -> Vec<Minor> {
    let n = weights.len();
    if n == 0 {
        return Vec::new();
    }
    // A negative weight is not a share of anything; it contributes nothing.
    let w: Vec<i128> = weights.iter().map(|x| (*x as i128).max(0)).collect();
    let w_total: i128 = w.iter().sum();
    if w_total == 0 {
        return vec![0; n]; // nothing to split it over — and no division by zero
    }

    let negative = total < 0;
    let mut magnitude = (total as i128).abs();
    if !negative && magnitude > w_total {
        magnitude = w_total; // a discount cannot exceed what it discounts
    }

    // Exact share = magnitude * w_i / w_total. Floor + remainder, both exact in integers.
    let mut parts: Vec<i128> = Vec::with_capacity(n);
    let mut remainders: Vec<(i128, usize)> = Vec::with_capacity(n);
    for (i, wi) in w.iter().enumerate() {
        let numerator = magnitude * wi;
        parts.push(numerator / w_total);
        remainders.push((numerator % w_total, i));
    }

    // Leftover cents → biggest fractional part first; equal remainders keep input order.
    let assigned: i128 = parts.iter().sum();
    let mut leftover = magnitude - assigned;
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (rem, i) in remainders {
        if leftover == 0 {
            break;
        }
        if rem == 0 {
            break; // no fractional part left to reward — and the sum already matches
        }
        parts[i] += 1;
        leftover -= 1;
    }

    parts.into_iter().map(|p| if negative { -(p as Minor) } else { p as Minor }).collect()
}

/// A quote line as the caller hands it over: an amount and the rate that applies to it.
///
/// The rate is what makes the allocation fiscally safe: allocating to lines allocates to **tax
/// rates**, which is the level at which the desglose (and VeriFactu's `DetalleDesglose`) lives.
struct QuoteLine {
    line_ref: String,
    amount: Minor,
    rate_pct: Decimal,
    basis: Option<&'static str>,
}

/// Reads `payload.lines`. `None` = the caller sent an aggregate amount (the legacy shape).
fn parse_quote_lines(payload: &Value) -> Result<Option<Vec<QuoteLine>>, String> {
    let arr = match payload.get("lines") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(a)) => a,
        Some(_) => return Err("invalid_payload: `lines` must be null or an array".to_string()),
    };
    let mut out = Vec::with_capacity(arr.len());
    for (i, l) in arr.iter().enumerate() {
        let line_ref = opt_str(l.get("line_ref")).unwrap_or_else(|| i.to_string());
        let amount = sdk_money::from_json(l.get("amount").unwrap_or(&Value::Null), 0);
        if amount < 0 {
            return Err("invalid_amount".to_string());
        }
        let rate_pct = dec_opt(l.get("tax_rate_pct")).unwrap_or(Decimal::ZERO);
        out.push(QuoteLine { line_ref, amount, rate_pct, basis: basis_from_mode(l.get("tax_basis")) });
    }
    Ok(Some(out))
}

/// The basis of a whole quote: the payload (or its lines) → the hub → the default.
///
/// Lines that disagree with each other, or with the payload, are **rejected**: mixing a gross and
/// a net amount in one order without a documented conversion is how a desglose stops squaring.
fn resolve_quote_basis(
    payload: &Value,
    context: &Value,
    lines: &[QuoteLine],
) -> Result<(&'static str, &'static str), String> {
    let mut declared: Option<&'static str> = basis_from_mode(payload.get("tax_basis"));
    for l in lines {
        if let Some(b) = l.basis {
            match declared {
                Some(prev) if prev != b => return Err("mixed_tax_basis".to_string()),
                _ => declared = Some(b),
            }
        }
    }
    if let Some(b) = declared {
        return Ok((b, "payload"));
    }
    if let Some(b) = basis_from_mode(context.get("tax_mode")) {
        return Ok((b, "hub"));
    }
    Ok((DEFAULT_TAX_BASIS, "default"))
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

    // ¿Ese código ya es de otra tarifa? (pricing#29)
    //
    // Antes esto lo decidía el INSERT estrellándose contra `uq_pricing_list_hub_code`, y el
    // usuario acababa leyendo «db: sqlx: … duplicate key value violates unique constraint … at
    // line 666». La unicidad sigue siendo del índice; lo que cambia es QUIÉN da la noticia.
    //
    // Aquí NO se delega en el `expect_rows` del manifest, y conviene saber por qué con precisión:
    //
    //   · Hasta hub#1071 (mergeado a `develop` el 2026-08-20) el camino WASM ejecutaba sus
    //     intenciones con `db.execute_tx`, NO con `execute_tx_gated`: el `expect_rows` de un
    //     sub-command alcanzado por un handler NUNCA se miraba (hub#1025). Desde #1071 sí, con una
    //     gate por operación.
    //   · Pero la flota corre imágenes PINEADAS A TAGS, así que ese arreglo todavía no está en
    //     producción. Una guarda que solo funcione en el runtime nuevo no arregla el hub de nadie.
    //
    // La comprobación en el handler funciona en LAS DOS versiones, y además es mejor donde importa:
    // rechaza ANTES de intentar escribir y puede nombrar el código que choca, cosa que un mensaje
    // de `expect_rows` —fijo en el manifest— no puede hacer. Es el mecanismo de hub#139/ADR-0205:
    // el host pre-carga la lectura, el handler devuelve `Output.error` y el host aborta antes de
    // aplicar nada, incluido el `_unset_default` de más abajo (si no, rechazar el alta dejaría al
    // hub sin tarifa por defecto).
    //
    // Y por eso `commands/price_list_create.sql` sigue SIN `ON CONFLICT`: en la carrera que le gane
    // a esta lectura, el índice tiene que negarse a escribir. Tragarse la fila en silencio sería
    // peor que el error crudo que esta issue vino a quitar.
    //
    // La lectura es `code_taken`, un espejo del índice: sin bloque `list` (un `reads` sobre una
    // query paginada solo trae la primera página, hub#650) y sin filtro de estado (una tarifa
    // retirada sigue siendo dueña de su código). Va declarada `required`, así que si no resuelve el
    // command se aborta en vez de degradar a un «no hay duplicado» que sería mentira.
    // Se compara el `code` de la fila en vez de fiarse de que la lectura venga filtrada. El
    // `reads` la filtra por `payload.code`, sí — pero si ese binding se rompiera, un handler que
    // solo mira «¿hay filas?» rechazaría TODAS las altas, y el fallo se leería como «ya existe»
    // sobre un código libre. Comparar aquí cuesta nada y hace que la guarda sea correcta por sí
    // misma. Sensible a mayúsculas, como el índice.
    let taken = preloaded_rows(&context, "pricing.price_lists.code_taken")
        .iter()
        .any(|row| as_str(row.get("code").unwrap_or(&Value::Null)).trim() == code);
    if taken {
        return Ok(Output::new().with_error(erplora_guest_sdk::DomainError::new(
            "pricing.duplicate_code",
            format!("A price list with the code `{code}` already exists. Pick a different code."),
        )));
    }

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
    // Tax basis of the list (ADR-0210), tri-state: 1 = prices already carry the tax, 0 = they are
    // the taxable base, NULL = inherit the hub. NULL is stored as NULL on purpose — resolving it
    // here would freeze today's hub setting into a list that should keep following the hub.
    p.insert(
        "tax_included".into(),
        match payload.get("tax_included") {
            None | Some(Value::Null) => Value::Null,
            Some(v) => json!(as_bool(v) as i64),
        },
    );
    ops.push(Operation::sql("pricing._insert_price_list", p));

    // El evento `pricing.price_list.created` lo emite el host (declarado en el
    // `emit` del command en module.json); el handler no lo duplica.
    // `..Default::default()` so the literal compiles against BOTH shapes of `Output`: the one
    // before hub#139 and the one that gained `error` (structured domain rejection). Without it the
    // handler stops compiling the moment the hub checkout moves on, and nobody can rebuild
    // `dist/handler.wasm` (pm#81).
    Ok(Output { operations: ops, events: vec![], ..Default::default() })
}

// ───────────────────────── §2 get_price (read-only) ─────────────────────────

/// Pure logic of `pricing.price_lists.get_price` (WASM-TODO.md §2, read-only).
///
/// * `payload` — `{product_ref, quantity?, price_list_id?, customer_segment?}`. `quantity` va en
///   **µ** (punto fijo entero, escala 10⁶ — ADR-0147 §2.1): una unidad es `1000000`. Ausente = una
///   unidad. Un decimal se **rechaza** (`invalid_quantity`), no se trunca.
/// * `context` — the host context; `tax_mode` is the hub-level fallback of the tax basis.
/// * `price_lists` — the hub's list rows (host preload: `pricing.price_lists.list`).
/// * `items` — the item rows for `product_ref` (host preload:
///   `pricing.price_lists.items_by_product`).
///
/// Returns the **lowest** price among the items whose quantity bracket matches, **plus the whole
/// contract that price needs to be usable** (ADR-0210): `tax_basis` and where it came from,
/// `currency`, and the currency's `minor_unit_decimals`. A bare integer is not a price: the same
/// `121` is a gross price on a retail list and a net price on a B2B one.
///
/// Errors: `invalid_quantity`, `invalid_id`, `no_price_list`, `no_price`, `mixed_tax_basis`.
pub fn get_price_compute(
    payload: &Value,
    context: &Value,
    price_lists: &[Value],
    items: &[Value],
) -> Result<Value, String> {
    let product_ref =
        as_str(payload.get("product_ref").unwrap_or(&Value::Null)).trim().to_string();
    if product_ref.is_empty() {
        return Err("invalid_payload: `product_ref` no puede estar vacío".to_string());
    }
    // La cantidad viaja en µ, como en todo el proyecto (ADR-0147 §2.1): `sales` manda `1_000_000`
    // para una unidad, no `1`. Ausente = UNA unidad, que en µ es `QUANTITY_SCALE` — no `1`, que
    // sería una millonésima.
    let quantity = match payload.get("quantity") {
        None | Some(Value::Null) => QUANTITY_SCALE,
        Some(v) => micro(v).ok_or_else(|| "invalid_quantity".to_string())?,
    };
    if quantity <= 0 {
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
    let mut matched_lists: Vec<String> = Vec::new();
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
        // Bracket en µ contra cantidad en µ: comparación de ENTEROS, sin coma flotante de por
        // medio. Mientras la columna fue `REAL`, esto comparaba 0,5 con 500000 sin dar error — el
        // bracket contestaba un precio creíble y equivocado por un factor de un millón.
        // Los dos extremos son INCLUSIVOS: «de 10 a 99» incluye el 10 y el 99.
        let min_q = micro_opt(it.get("min_quantity")).unwrap_or(QUANTITY_SCALE);
        if quantity < min_q {
            continue;
        }
        if let Some(max_q) = micro_opt(it.get("max_quantity")) {
            if quantity > max_q {
                continue;
            }
        }
        let raw_price = it.get("price").cloned().unwrap_or(Value::Null);
        let price = match dec(&raw_price) {
            Some(p) => p,
            None => continue,
        };
        if !matched_lists.contains(&list_id) {
            matched_lists.push(list_id.clone());
        }
        if best.as_ref().map(|(b, _, _)| price < *b).unwrap_or(true) {
            best = Some((price, raw_price, list_id));
        }
    }

    // NO IMPLICIT CONVERSION between bases (ADR-0210). Picking "the cheapest" between a gross 121
    // and a net 100 is comparing two different magnitudes; the caller has to say which list it
    // wants instead of getting a number whose meaning depends on which row happened to win.
    let mut basis: Option<(&'static str, &'static str)> = None;
    for id in &matched_lists {
        let row = price_lists
            .iter()
            .find(|l| as_str(l.get("id").unwrap_or(&Value::Null)) == *id)
            .cloned()
            .unwrap_or(Value::Null);
        let resolved = resolve_tax_basis(&row, context);
        match basis {
            Some(prev) if prev.0 != resolved.0 => return Err("mixed_tax_basis".to_string()),
            _ => basis = Some(resolved),
        }
    }

    match best {
        Some((_, raw_price, list_id)) => {
            let (tax_basis, basis_source) = basis.unwrap_or((DEFAULT_TAX_BASIS, "default"));
            // The currency is the hub's (ADR-0123 §1), carried on the list row so the caller never
            // has to assume 2 decimals: 1499 is 14,99 € but 1499 ¥.
            let currency = price_lists
                .iter()
                .find(|l| as_str(l.get("id").unwrap_or(&Value::Null)) == list_id)
                .and_then(|l| opt_str(l.get("currency")))
                .unwrap_or_else(|| "EUR".to_string());
            let decimals = sdk_currency::decimals_for(&currency).unwrap_or(2);
            Ok(json!({
                "product_ref": product_ref,
                // Money is an INTEGER of minor units (ADR-0123) — normalised here so the caller
                // never receives whatever shape the row happened to carry.
                "price": sdk_money::from_json(&raw_price, 0),
                // µ, entero — el mismo lenguaje que habla `sale.completed` (ADR-0147 §2.1). Antes
                // salía como la cadena de la cantidad lógica ("0.5"), que obligaba al llamante a
                // reescalar a mano justo donde se multiplica el dinero.
                "quantity": quantity,
                "price_list_id": list_id,
                "currency": currency,
                "minor_unit_decimals": decimals,
                "tax_basis": tax_basis,
                "tax_basis_source": basis_source,
            }))
        }
        None => Err("no_price".to_string()),
    }
}

// ─────────────────────── §3 calculate_discount (read-only) ───────────────────────

/// Pure logic of `pricing.rules.calculate_discount` (WASM-TODO.md §3, read-only).
///
/// * `payload` — `{amount?, lines?, tax_basis?, rules?, customer_segment?}`. `rules` accepts null
///   (all active ones), a list of ids, a list of inline dicts, or a mix.
/// * `context` — the host context; `tax_mode` is the hub-level fallback of the tax basis.
/// * `db_rules` — the hub's active rules preloaded by the host (`pricing.rules.list`); inline
///   ones arrive in the payload.
///
/// Rules apply by ascending `priority` (ties by `code`) over the running total, clamped to
/// `[0, running]` and rounded to a whole cent.
///
/// # Two shapes, and only one of them is safe for a fiscal document (ADR-0210)
///
/// * `{amount}` — a single aggregate. Fine for a preview ("how much would this coupon save?"),
///   **not** for a sale: an aggregate discount over an aggregate amount carries no lines and no
///   tax rates, so whoever receives it has to split it by hand — and that is the split that
///   silently unsquares the desglose.
/// * `{lines}` — the amounts with their rates. The reply then also carries `allocation` (per
///   line, adding up to `total_discount` **exactly**) and `by_tax_rate` (the same thing rolled up
///   to the level the desglose actually lives at, ADR-0123 §4).
///
/// Order of calculation, fixed: **price → discount (allocated) → base/tax per rate → rounding**.
///
/// Errors: `invalid_amount`, `amount_mismatch`, `mixed_tax_basis`.
pub fn calculate_discount_compute(
    payload: &Value,
    context: &Value,
    db_rules: &[Value],
) -> Result<Value, String> {
    let quote_lines = parse_quote_lines(payload)?;
    let (tax_basis, basis_source) =
        resolve_quote_basis(payload, context, quote_lines.as_deref().unwrap_or(&[]))?;

    // With lines, the amount IS the lines. An aggregate that disagrees with them is two sources of
    // truth for one number — rejected instead of silently picking one.
    let amount = match &quote_lines {
        Some(lines) => {
            let derived: Minor = lines.iter().map(|l| l.amount).sum();
            if let Some(given) = payload.get("amount") {
                if !given.is_null() && sdk_money::from_json(given, derived) != derived {
                    return Err("amount_mismatch".to_string());
                }
            }
            Decimal::from(derived)
        }
        None => dec_opt(payload.get("amount")).ok_or_else(|| "invalid_amount".to_string())?,
    };
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

    let total_discount = sdk_money::round(original - running);
    let mut out = json!({
        "original_amount": money(original),
        "final_amount": money(running),
        "total_discount": total_discount,
        "applied_rules": applied,
        "tax_basis": tax_basis,
        "tax_basis_source": basis_source,
    });

    // ── The allocation: the discount reaches the LINES, and through them the tax rates ──
    if let Some(lines) = &quote_lines {
        let weights: Vec<Minor> = lines.iter().map(|l| l.amount).collect();
        let parts = allocate_amount(total_discount, &weights);

        let allocation: Vec<Value> = lines
            .iter()
            .zip(parts.iter())
            .map(|(l, d)| {
                json!({
                    "line_ref": l.line_ref,
                    "amount": l.amount,
                    "discount": d,
                    "amount_after": l.amount - d,
                    "tax_rate_pct": l.rate_pct.to_f64().unwrap_or(0.0),
                })
            })
            .collect();

        // Rolled up per TAX RATE, in order of first appearance: that is the level the desglose
        // lives at (one `DetalleDesglose` per rate, ADR-0123 §4), and the level at which "the
        // discount did not unsquare anything" is a checkable invariant.
        let mut by_rate: Vec<(Decimal, Minor, Minor)> = Vec::new(); // (rate, before, discount)
        for (l, d) in lines.iter().zip(parts.iter()) {
            match by_rate.iter_mut().find(|(r, _, _)| *r == l.rate_pct) {
                Some(e) => {
                    e.1 += l.amount;
                    e.2 += d;
                }
                None => by_rate.push((l.rate_pct, l.amount, *d)),
            }
        }
        let by_tax_rate: Vec<Value> = by_rate
            .iter()
            .map(|(rate, before, disc)| {
                json!({
                    "tax_rate_pct": rate.to_f64().unwrap_or(0.0),
                    "amount_before": before,
                    "discount": disc,
                    "amount_after": before - disc,
                })
            })
            .collect();

        out["allocation"] = json!(allocation);
        out["by_tax_rate"] = json!(by_tax_rate);
    }

    Ok(out)
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
// Test names SHOUT the part that matters (`..._EXACTLY`, `..._NOT_5`): the emphasis is the point
// of the name, and a failing test should say what broke without opening the file.
#[allow(non_snake_case)]
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

    /// A price list item row (`pricing.price_lists.items_by_product`). Quantities travel in µ
    /// (fixed point, scale 10⁶ — ADR-0147), exactly as the column stores them.
    fn price_item(list_id: &str, product_ref: &str, price: i64) -> Value {
        json!({
            "id": format!("{list_id}-{product_ref}"), "price_list_id": list_id,
            "product_ref": product_ref, "price": price,
            "min_quantity": QUANTITY_SCALE, "max_quantity": Value::Null, "is_deleted": 0,
        })
    }

    /// The same row with an explicit quantity BRACKET, in µ.
    fn price_item_bracket(list_id: &str, price: i64, min_micro: i64, max_micro: Option<i64>) -> Value {
        json!({
            "id": format!("{list_id}-{min_micro}"), "price_list_id": list_id,
            "product_ref": "P1", "price": price,
            "min_quantity": min_micro,
            "max_quantity": max_micro.map(|m| json!(m)).unwrap_or(Value::Null),
            "is_deleted": 0,
        })
    }

    /// `get_price` with the quantity in µ, against one list.
    fn price_for(quantity_micro: i64, items: &[Value]) -> Result<Value, String> {
        get_price_compute(
            &json!({ "product_ref": "P1", "quantity": quantity_micro }),
            &json!({}),
            &[price_list("L1", Some(true), "EUR")],
            items,
        )
    }

    // ─────────────── the quantity is µ, not a float (ADR-0147, pricing#22) ───────────────
    //
    // `min_quantity`/`max_quantity` were the last `REAL` columns of the project. Comparing a REAL
    // against an integer of scale 10⁶ does not raise: it silently answers a bracket that is off by
    // a factor of a million, which is the worst kind of bug — a believable wrong price.

    #[test]
    fn HALF_A_KILO_is_500000_and_matches_the_bracket_that_starts_at_HALF_A_KILO() {
        // 0,5 kg exactly ON the lower edge of the bracket: `min_quantity` is inclusive.
        let items = vec![price_item_bracket("L1", 900, 500_000, None)];
        let out = price_for(500_000, &items).expect("0,5 kg is inside [0,5 kg, ∞)");
        assert_eq!(out["price"], json!(900));
        assert_eq!(out["quantity"], json!(500_000), "the answer speaks µ, like the rest of the project");
    }

    #[test]
    fn a_HAIR_UNDER_the_bracket_does_NOT_match_it() {
        // 0,499999 kg — one µ below the edge. With floats this is where the rounding lies start.
        let items = vec![price_item_bracket("L1", 900, 500_000, None)];
        assert_eq!(price_for(499_999, &items), Err("no_price".to_string()));
    }

    #[test]
    fn the_UPPER_edge_of_a_bracket_is_inclusive_too_and_the_next_micro_falls_out() {
        let items = vec![price_item_bracket("L1", 900, 1, Some(2_500_000))];
        assert!(price_for(2_500_000, &items).is_ok(), "2,5 is the last quantity of the bracket");
        assert_eq!(price_for(2_500_001, &items), Err("no_price".to_string()));
    }

    #[test]
    fn TIERS_pick_the_bracket_the_quantity_falls_in_NOT_the_cheapest_row() {
        // 1-9 → 1,00 € · 10-99 → 0,90 € · 100+ → 0,80 €. Asking for 10 must answer 90, not 80:
        // "the lowest price among the MATCHING items" is only sound if the matching is exact.
        let items = vec![
            price_item_bracket("L1", 100, 1_000_000, Some(9_999_999)),
            price_item_bracket("L1", 90, 10_000_000, Some(99_999_999)),
            price_item_bracket("L1", 80, 100_000_000, None),
        ];
        assert_eq!(price_for(1_000_000, &items).unwrap()["price"], json!(100));
        assert_eq!(price_for(10_000_000, &items).unwrap()["price"], json!(90));
        assert_eq!(price_for(100_000_000, &items).unwrap()["price"], json!(80));
    }

    #[test]
    fn ONE_unit_is_1_000_000_so_a_bare_1_is_a_MILLIONTH_and_matches_nothing() {
        // The guard against the regression this issue is about: if some caller still speaks logical
        // units, it must FAIL LOUDLY (no bracket matches) instead of quietly buying at tier price.
        let items = vec![price_item_bracket("L1", 90, 10_000_000, None)];
        assert_eq!(
            price_for(1, &items),
            Err("no_price".to_string()),
            "a bare `1` is one MILLIONTH of a unit — it can never reach a 10-unit tier"
        );
    }

    #[test]
    fn a_quantity_of_ZERO_or_less_is_still_rejected() {
        let items = vec![price_item("L1", "P1", 100)];
        assert_eq!(price_for(0, &items), Err("invalid_quantity".to_string()));
        assert_eq!(price_for(-1_000_000, &items), Err("invalid_quantity".to_string()));
    }

    #[test]
    fn a_FRACTIONAL_quantity_is_refused_at_the_door_it_is_not_truncated() {
        // µ is an INTEGER contract. `0.5` is not "half a µ", it is a caller that never got the
        // memo — and truncating it is exactly how `as_i64` turned 0,5 kg into 0 (ADR-0147 §1).
        let items = vec![price_item("L1", "P1", 100)];
        let out = get_price_compute(
            &json!({ "product_ref": "P1", "quantity": 0.5 }),
            &json!({}),
            &[price_list("L1", Some(true), "EUR")],
            &items,
        );
        assert_eq!(out, Err("invalid_quantity".to_string()));
    }

    #[test]
    fn an_item_WITHOUT_a_bracket_defaults_to_ONE_UNIT_in_micro_not_to_a_bare_1() {
        // A row whose `min_quantity` is absent must behave as "from 1 unit", i.e. 1_000_000 µ.
        let bare = json!({
            "id": "x", "price_list_id": "L1", "product_ref": "P1", "price": 100,
            "is_deleted": 0,
        });
        assert!(price_for(1_000_000, &[bare.clone()]).is_ok(), "1 unit must match the default floor");
        assert_eq!(
            price_for(999_999, &[bare]),
            Err("no_price".to_string()),
            "a hair under one unit is below the default floor of 1 unit"
        );
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

    // ─────────────── pricing#29 · a repeated code is a DOMAIN error, not a driver crash ───────────────
    //
    // The shop owner used to read this, in red, at the top of the screen:
    //
    //     db: sqlx: error returned from database: duplicate key value violates unique constraint
    //     "uq_pricing_list_hub_code" at line 666
    //
    // `expect_rows` cannot fix this one: the WASM path runs its intentions through
    // `db.execute_tx`, not `execute_tx_gated`, so a guard on the private `_insert_price_list`
    // would never run. The rejection has to come from here.

    /// Input for `create_price_list_pure` with the `code_taken` read already preloaded by the host.
    fn create_input(code: &str, taken: &[&str]) -> Value {
        let rows: Vec<Value> = taken
            .iter()
            .map(|c| json!({ "id": format!("existing-{c}"), "code": c }))
            .collect();
        json!({
            "payload": { "code": code, "name": "Tarifa general" },
            "context": {
                "new_ids": ["list-1"],
                "reads": { "pricing.price_lists.code_taken": rows },
            },
        })
    }

    #[test]
    fn a_repeated_code_is_rejected_with_a_stable_namespaced_code() {
        let out = create_price_list_pure(create_input("PVP", &["PVP"])).expect("no panic");
        let err = out.error.as_ref().expect("the duplicate must be REJECTED, not written");
        assert_eq!(err.code, "pricing.duplicate_code");
    }

    #[test]
    fn the_rejection_names_the_offending_code_and_leaks_nothing_of_the_engine() {
        let out = create_price_list_pure(create_input("PVP", &["PVP"])).expect("no panic");
        let message = out.error.as_ref().expect("rejected").message.to_lowercase();
        // It has to be actionable: the owner must know WHICH code clashed.
        assert!(message.contains("pvp"), "the message must name the code: {message}");
        // And it must not republish the insides the way the raw error did.
        for leak in ["sqlx", "constraint", "db:", "uq_pricing", "line 666"] {
            assert!(!message.contains(leak), "the message leaks `{leak}`: {message}");
        }
    }

    #[test]
    fn a_rejected_creation_writes_NOTHING_at_all() {
        // Including the `_unset_default` that would otherwise run first: rejecting the creation
        // must not leave the hub without a default tariff.
        let mut input = create_input("PVP", &["PVP"]);
        input["payload"]["is_default"] = json!(true);
        let out = create_price_list_pure(input).expect("no panic");
        assert!(out.error.is_some(), "rejected");
        assert!(
            out.operations.is_empty(),
            "a rejected creation emitted operations: {:?}",
            out.operations
        );
        assert!(out.events.is_empty(), "a rejected creation emitted events");
    }

    #[test]
    fn a_free_code_is_still_created_normally() {
        // The guard must reject DUPLICATES, not creations. Without this, a handler that always
        // returned the error would pass every test above.
        let out = create_price_list_pure(create_input("PVP2", &["PVP"])).expect("no panic");
        assert!(out.error.is_none(), "a free code was rejected: {:?}", out.error);
        let insert = out.operations.last().expect("insert op");
        assert_eq!(insert.params["code"], json!("PVP2"));
    }

    #[test]
    fn an_empty_read_means_the_code_is_free() {
        let out = create_price_list_pure(create_input("PVP", &[])).expect("no panic");
        assert!(out.error.is_none(), "no rows preloaded = nothing taken");
        assert!(!out.operations.is_empty(), "it should have created the list");
    }
}
