-- El dinero es un INTEGER de CÉNTIMOS (ADR-0007). `pricing_discount_rule` se quedó fuera: su
-- columna `value REAL` era POLIMÓRFICA — «% o euros», según lo que dijera OTRA columna
-- (`rule_type`). Una columna, dos unidades. El resultado: un descuento fijo de 5 € se guardaba
-- como `5.0` y el motor lo restaba de un importe en céntimos → descontaba 5 CÉNTIMOS.
--
-- Se separan las dos unidades para que ninguna sea ambigua:
--   · `value`        → TASA en % (percent, y tiered con unit=percent). NO es dinero.
--   · `amount_cents` → DINERO en céntimos (fixed). Entero, como el resto del hub.
ALTER TABLE pricing_discount_rule ADD COLUMN IF NOT EXISTS amount_cents INTEGER;

-- Backfill: las reglas `fixed` que ya existan guardan EUROS en `value` → pasan a céntimos.
-- `ROUND` antes de `CAST` para no truncar (5.99 € → 599, no 598).
UPDATE pricing_discount_rule
   SET amount_cents = CAST(ROUND(value * 100) AS INTEGER)
 WHERE rule_type = 'fixed';

-- En esas filas `value` ya no significa nada: se pone a 0 para que nadie la vuelva a leer como
-- dinero (el bug era exactamente eso).
UPDATE pricing_discount_rule
   SET value = 0
 WHERE rule_type = 'fixed';
