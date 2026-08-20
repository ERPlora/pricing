-- Alta de regla de descuento. Runtime inyecta :new_id, :hub_id, :current_user_id, :now.
-- Portado de PricingService.create_discount_rule. La validación de rule_type contra el set
-- {percent,fixed,buy_x_get_y,tiered} la fija el schema JSON; :conditions llega como texto JSON.
--
-- Las dos unidades van SEPARADAS (ADR-0007/0123, migración 002): `value` es la TASA en % y
-- `amount_cents` es DINERO en céntimos (el importe de una regla `fixed`). Nunca al revés: antes
-- compartían la columna `value REAL -- % o euros` y un descuento fijo de 5 € restaba 5 CÉNTIMOS.
INSERT INTO pricing_discount_rule
  (id, hub_id, code, name, rule_type, value, amount_cents, min_amount, max_amount, applies_to, conditions,
   valid_from, valid_until, is_active, priority,
   is_deleted, created_by, updated_by, created_at, updated_at)
VALUES
  (:new_id, :hub_id, :code, :name, :rule_type, :value, :amount_cents, :min_amount, :max_amount, :applies_to, :conditions,
   :valid_from, :valid_until, 1, :priority,
   0, :current_user_id, :current_user_id, :now, :now)
-- Un código repetido es un error de NEGOCIO, no una excepción del driver (pricing#29). El índice
-- único `uq_pricing_rule_hub_code` sigue siendo la autoridad —es lo que garantiza la unicidad bajo
-- concurrencia—, pero el INSERT deja de estrellarse contra él: la colisión se convierte en CERO
-- filas escritas, y de ahí la recoge el `expect_rows` del manifest, que la devuelve como
-- `pricing.duplicate_code` (HTTP 409) con un mensaje que el usuario puede leer.
-- Las columnas del ON CONFLICT son EXACTAMENTE las del índice: ni más (dejaría pasar duplicados)
-- ni menos (rechazaría altas legítimas de otro hub).
ON CONFLICT (hub_id, code) DO NOTHING;
