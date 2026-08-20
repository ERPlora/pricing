// rule-value — lo que la columna VALOR de «Reglas de descuento» enseña (pricing#28).
//
// La columna estaba atada a UN solo campo, `value`, y el modelo tiene DOS a propósito
// (ADR-0007/0123, migración 002): `value` es una TASA en % y `amount_cents` es DINERO en céntimos.
// El propio schema del command lo avisa: antes compartían la columna `value` («% o euros» según
// `rule_type`) y un descuento fijo de 5 € acababa restando 5 CÉNTIMOS. El dato se separó bien y la
// columna se quedó atrás: una regla `fixed` tiene `value = 0.0`, así que la tabla decía que una
// regla que resta 5,00 € de cada ticket valía **0**.
//
// Y la misma columna mezclaba dos unidades sin decir cuál era cuál: el `10` de una fila eran POR
// CIENTO y el `0` de la otra pretendían ser EUROS, sin `%` ni `€` en ninguna.
//
// ── Por qué UNA columna formateada y no dos («Descuento %» y «Descuento €») ──────────────────
//
// La issue dejaba las dos opciones abiertas. Decide el mercado (regla del monorepo, 2026-08-14):
//
//   | Producto            | Cómo lo resuelve                                            |
//   |---------------------|-------------------------------------------------------------|
//   | Square              | UNA columna «Amount», con `$5.00` o `10%` según el tipo      |
//   | Shopify             | UNA, la lista de Discounts lee «10% off» / «$5 off»          |
//   | Toast               | UNA; el tipo es «Fixed $ Off» / «Fixed % Off»                |
//   | Clover              | UNA; al crear se elige el signo `$` o `%` en el mismo campo  |
//   | Lightspeed          | UNA («Open Amount» / «Open Percentage» son tipos, no columnas)|
//   | WooCommerce         | UNA («Coupon amount»); el tipo dice la unidad                |
//   | Odoo                | UNA por regla; el «Price Type» decide qué significa          |
//   | Business Central    | DOS campos (`Line Discount %` / `Line Discount Amount`)      |
//   | Sage 200            | DOS campos («Porcentaje» / «Valor de la transacción»)        |
//
// Gana UNA columna con su unidad. Los dos que separan (BC y Sage) lo hacen sobre la LÍNEA de un
// documento, donde los dos valores COEXISTEN de verdad: un % de descuento produce además un
// importe, y las dos cifras son ciertas a la vez. Aquí no: una regla es `percent` XOR `fixed`, así
// que dos columnas dejarían una siempre vacía en cada fila — el mismo hueco del `0`, disfrazado— y
// ensancharían la tabla en el viewport de 390 px. Desempate del encargo: gana lo más rápido de
// leer y lo que cabe en una mano.
//
// ── Orden y filtro: se QUITAN de esta columna ────────────────────────────────────────────────
//
// Ordenar una columna que mezcla unidades no significa nada (¿es 10 % mayor que 5 €?), y el filtro
// `range` sobre `value` buscaba en POR CIENTO, así que un «≥ 5» no encontraba la regla de 5 €. Es
// un antipatrón conocido de tablas: una columna con unidades mezcladas rompe el orden y engaña.
// El eje por el que de verdad se agrupa —`rule_type`— sigue siendo ordenable y filtrable, y
// `priority`, que es lo que decide qué regla gana, también.

/** Fila de `pricing.rules.list`, tal como llega del wire (los números pueden venir como texto). */
export interface DiscountRuleLike {
  rule_type?: string;
  /** TASA en % (reglas `percent`). Vale `0` en una regla `fixed`. */
  value?: number | string;
  /** DINERO en céntimos (reglas `fixed`). Vale `0` en una regla `percent`. */
  amount_cents?: number | string;
}

/** Clave i18n del marcador para las reglas cuyo valor no es un número suelto. */
const NO_SINGLE_VALUE_KEY = 'ui.ruleValueNotApplicable';
/** Lo que se pinta si el catálogo no tiene esa clave. Un guion se entiende en cualquier idioma. */
const DASH = '—';

function num(v: number | string | undefined): number {
  const n = typeof v === 'string' ? Number(v) : v;
  return Number.isFinite(n) ? (n as number) : 0;
}

/**
 * Texto de la columna VALOR de una regla de descuento: **su magnitud con su unidad**.
 *
 *     percent      → «10 %»      (de `value`)
 *     fixed        → «5,00 €»    (de `amount_cents`, vía `formatMoney`, que recibe CÉNTIMOS)
 *     buy_x_get_y  → «—»         (su valor vive en `conditions`, no se resume en un número)
 *     tiered       → «—»         (idem)
 *
 * Un tipo desconocido cae también en «—»: inventarle un número sería repetir el bug — un `0`
 * concreto y falso es peor que un guion honesto.
 *
 * @param formatMoney el del SDK; recibe **céntimos** y aplica la moneda del hub (ADR-0007).
 * @param t           traductor del módulo, ya atado a su catálogo.
 */
export function ruleValueLabel(
  rule: DiscountRuleLike,
  formatMoney: (cents: number) => string,
  t: (key: string) => string,
): string {
  switch (String(rule.rule_type ?? '')) {
    case 'fixed':
      return formatMoney(num(rule.amount_cents));
    case 'percent':
      return `${num(rule.value)} %`;
    default: {
      const label = t(NO_SINGLE_VALUE_KEY);
      return label && label !== NO_SINGLE_VALUE_KEY ? label : DASH;
    }
  }
}
