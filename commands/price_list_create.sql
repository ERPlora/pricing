-- Alta de lista de precios. Lo invoca el handler WASM `create_price_list` como
-- intención `pricing._insert_price_list` (command privado), DESPUÉS de
-- `pricing._unset_default` cuando la nueva lista es default — ambos en la misma
-- transacción (invariante "una sola lista default por hub", WASM-TODO.md §1).
-- El id (:price_list_id) lo reparte el handler desde context.new_ids (el host es
-- la autoridad de ids); :hub_id, :current_user_id y :now los inyecta el runtime.
-- Portado de PricingService.create_price_list.
INSERT INTO pricing_price_list
  (id, hub_id, code, name, currency, is_default, is_active, valid_from, valid_until, segment,
   is_deleted, created_by, updated_by, created_at, updated_at)
VALUES
  (:price_list_id, :hub_id, :code, :name, :currency, :is_default, 1, :valid_from, :valid_until, :segment,
   0, :current_user_id, :current_user_id, :now, :now);
