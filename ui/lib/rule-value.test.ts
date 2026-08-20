// Contrato de la columna VALOR de «Reglas de descuento» (pricing#28).
//
// La tabla decía esto:
//
//   | CÓDIGO | NOMBRE       | TIPO         | VALOR | PRIORIDAD |
//   | R10    | 10% general  | Porcentaje   |  10   | 100       |
//   | R5E    | 5 EUR fijo   | Importe fijo |   0   |  50       |
//
// La regla `R5E` descuenta 5,00 € de verdad (`calculate_discount` lo confirma), y la tabla decía
// que valía **0**. Quien administra los descuentos veía una regla aparentemente inofensiva que sí
// estaba restando dinero de cada ticket.
//
// La causa: el modelo tiene DOS campos a propósito (ADR-0007/0123, migración 002) — `value` es una
// TASA en % y `amount_cents` es DINERO en céntimos — y la columna se ató a `value` a secas. Para
// una regla `fixed`, `value` vale 0.0 y el dinero está en `amount_cents`. El dato se separó bien y
// la columna se quedó atrás.
//
// Y de paso la misma columna mezclaba dos unidades sin decir cuál era cuál: el `10` de arriba son
// POR CIENTO y el `0` de abajo pretendían ser EUROS. Ni `%` ni `€` en ninguna fila.

import { describe, expect, it } from 'vitest';
import { ruleValueLabel } from './rule-value';

/** `formatMoney` real del SDK: recibe CÉNTIMOS y divide. */
const money = (cents: number): string => `${((cents || 0) / 100).toFixed(2)} €`;
/** `t` que devuelve la clave — lo que hace el catálogo cuando le falta la entrada. */
const rawT = (k: string): string => k;

describe('cada fila enseña su magnitud CON su unidad (pricing#28)', () => {
  it('una regla `fixed` de 500 céntimos dice 5,00 € — no 0', () => {
    expect(ruleValueLabel({ rule_type: 'fixed', value: 0, amount_cents: 500 }, money, rawT))
      .toBe('5.00 €');
  });

  it('una regla `percent` de 10 dice 10 %', () => {
    expect(ruleValueLabel({ rule_type: 'percent', value: 10, amount_cents: 0 }, money, rawT))
      .toBe('10 %');
  });

  it('ninguna de las dos filas del ejemplo sale como un número desnudo', () => {
    const filas = [
      { rule_type: 'percent', value: 10, amount_cents: 0 },
      { rule_type: 'fixed', value: 0, amount_cents: 500 },
    ];
    for (const r of filas) {
      const label = ruleValueLabel(r, money, rawT);
      expect(label, `«${label}» no dice su unidad`).toMatch(/[%€]/);
    }
  });

  it('un `fixed` lee amount_cents y NUNCA value (que es lo que pasaba)', () => {
    // `value` contaminado a propósito: si la columna volviera a leerlo, saldría 99.
    expect(ruleValueLabel({ rule_type: 'fixed', value: 99, amount_cents: 500 }, money, rawT))
      .toBe('5.00 €');
  });

  it('un `percent` lee value y NUNCA amount_cents', () => {
    expect(ruleValueLabel({ rule_type: 'percent', value: 10, amount_cents: 99_999 }, money, rawT))
      .toBe('10 %');
  });

  it('un `fixed` de 0 céntimos sigue diciendo su unidad (0,00 €, no un 0 pelado)', () => {
    expect(ruleValueLabel({ rule_type: 'fixed', value: 0, amount_cents: 0 }, money, rawT))
      .toBe('0.00 €');
  });

  it('los céntimos llegan como texto desde el wire y siguen valiendo lo mismo', () => {
    expect(ruleValueLabel({ rule_type: 'fixed', value: '0', amount_cents: '500' }, money, rawT))
      .toBe('5.00 €');
  });

  it('un porcentaje con decimales se pinta tal cual, sin redondeos inventados', () => {
    expect(ruleValueLabel({ rule_type: 'percent', value: 7.5, amount_cents: 0 }, money, rawT))
      .toBe('7.5 %');
  });
});

describe('los tipos cuyo valor NO vive en ninguna de las dos columnas (pricing#28)', () => {
  // `buy_x_get_y` y `tiered` guardan lo suyo dentro de `conditions` (JSON). Hoy salían como `0`,
  // que es una cifra concreta y falsa. Un guion dice «esto no se resume en un número» y manda a
  // mirar la regla, que es lo cierto.
  it('buy_x_get_y no finge un número', () => {
    const label = ruleValueLabel({ rule_type: 'buy_x_get_y', value: 0, amount_cents: 0 }, money, rawT);
    expect(label).toBe('—');
    expect(label).not.toContain('0');
  });

  it('tiered tampoco', () => {
    expect(ruleValueLabel({ rule_type: 'tiered', value: 0, amount_cents: 0 }, money, rawT)).toBe('—');
  });

  it('un rule_type desconocido degrada al guion en vez de mentir', () => {
    expect(ruleValueLabel({ rule_type: 'algo_nuevo', value: 3, amount_cents: 0 }, money, rawT))
      .toBe('—');
  });

  it('sin rule_type tampoco se inventa nada', () => {
    expect(ruleValueLabel({ value: 3, amount_cents: 0 }, money, rawT)).toBe('—');
  });
});
