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
   0, :current_user_id, :current_user_id, :now, :now);
