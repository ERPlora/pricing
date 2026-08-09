-- Un item de tarifa ya no puede colgar de la lista de OTRO hub (pricing#10).
--
-- `pricing_price_list_item` guardaba `hub_id` y `price_list_id` por separado, y la FK apuntaba
-- solo al `id` del padre:
--
--     FOREIGN KEY (price_list_id) REFERENCES pricing_price_list (id)
--
-- Así que una fila del hub A podía apuntar perfectamente a una lista del hub B: la base de datos
-- lo aceptaba porque el `id` existía, sin mirar de quién era. El resultado es una relación que
-- ninguna pantalla puede enseñar (la lista no sale en el hub del item) y que ningún informe puede
-- cuadrar. El command también lo permitía; eso se cierra aparte, en `price_item_add.sql`.
--
-- Dos piezas, en este orden:
--
-- 1) La clave única `(hub_id, id)` en el padre. Es trivialmente cierta —`id` ya es PRIMARY KEY—,
--    pero Postgres exige que exista para poder apuntarla desde una FK compuesta.
ALTER TABLE pricing_price_list
    ADD CONSTRAINT pricing_price_list_hub_id_key UNIQUE (hub_id, id);

-- 2) La FK compuesta, que ya lleva el hub dentro. A partir de aquí, un item cuyo `hub_id` no
--    coincida con el de su lista lo rechaza el motor, no el módulo.
--
--    `NOT VALID` es deliberado. Sin él, Postgres valida TODAS las filas existentes al añadir la
--    restricción, y una sola fila cross-tenant heredada —justo las que este arreglo persigue—
--    haría fallar la migración y, con ella, la instalación del módulo. O sea: convertiría un
--    arreglo de seguridad en una migración destructiva (habría que borrar datos para poder
--    instalar). Con `NOT VALID` la puerta queda cerrada para toda fila NUEVA, que es el vector,
--    y sanear lo viejo es una operación consciente y aparte:
--
--        ALTER TABLE pricing_price_list_item VALIDATE CONSTRAINT pricing_price_list_item_hub_fk;
--
--    Antes de validar hay que mirar qué hay, no ejecutarlo a ciegas:
--
--        SELECT i.id, i.hub_id, i.price_list_id, l.hub_id AS list_hub_id
--        FROM pricing_price_list_item i
--        JOIN pricing_price_list l ON l.id = i.price_list_id
--        WHERE l.hub_id <> i.hub_id;
--
-- La FK vieja se retira porque la nueva la subsume (misma columna padre, más el hub).
ALTER TABLE pricing_price_list_item
    DROP CONSTRAINT IF EXISTS pricing_price_list_item_price_list_id_fkey;

ALTER TABLE pricing_price_list_item
    ADD CONSTRAINT pricing_price_list_item_hub_fk
    FOREIGN KEY (hub_id, price_list_id) REFERENCES pricing_price_list (hub_id, id)
    ON DELETE CASCADE
    NOT VALID;
