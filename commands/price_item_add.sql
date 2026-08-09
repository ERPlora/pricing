-- Alta de item (precio por producto/bracket) en una lista. Runtime inyecta :new_id, :hub_id,
-- :current_user_id, :now. Portado de PricingService.add_price_item.
--
-- El INSERT es CONDICIONAL a propósito (pricing#10). Antes era directo: metía el
-- `:price_list_id` que llegara, con el `:hub_id` del runtime al lado, sin comprobar de quién era
-- la lista. Un caller con un id de otro hub creaba una fila del hub A colgando de una lista del
-- hub B — invisible en las dos pantallas y sin forma de cuadrarla.
--
-- El padre se resuelve aquí dentro, en la misma sentencia y contra el `:hub_id` INYECTADO (no
-- contra un campo del payload, que el llamante controla). Además tiene que estar vivo y activa:
-- una lista borrada o desactivada no admite items nuevos.
--
-- Si la lista no es tuya, no existe, está borrada o inactiva, el SELECT no devuelve fila y el
-- INSERT afecta 0 filas. Eso NO es un éxito silencioso: el command declara
-- `expect_rows: {op: min, n: 1}`, así que el runtime revierte la transacción entera —ni fila ni
-- evento `pricing.price_item.added`— y devuelve el código de dominio `pricing.price_list_unavailable`.
INSERT INTO pricing_price_list_item
  (id, hub_id, price_list_id, product_ref, price, min_quantity, max_quantity,
   is_deleted, created_by, updated_by, created_at, updated_at)
SELECT
  :new_id, :hub_id, l.id, :product_ref, :price, :min_quantity, :max_quantity,
  0, :current_user_id, :current_user_id, :now, :now
FROM pricing_price_list l
WHERE l.id = :price_list_id
  AND l.hub_id = :hub_id
  AND l.is_deleted = 0
  AND l.is_active = 1;
