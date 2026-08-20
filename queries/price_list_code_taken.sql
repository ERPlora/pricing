-- ¿Está el código YA usado por otra tarifa de este hub? (pricing#29)
--
-- Esta query es un ESPEJO DEL ÍNDICE `uq_pricing_list_hub_code (hub_id, code)`, no una vista de
-- negocio, y por eso NO filtra `is_active` ni `is_deleted`: el índice tampoco lo hace, así que una
-- tarifa retirada sigue siendo dueña de su código. Filtrar aquí por estado haría que el handler
-- diera por libre un código que el INSERT va a rechazar — el error crudo volvería por esa puerta.
--
-- Tampoco declara bloque `list`: la carga `reads` de una query paginada entrega solo la PRIMERA
-- PÁGINA (hub#650), y a partir de 50 tarifas la comprobación empezaría a perder duplicados sin
-- avisar. El resultado es como mucho una fila (lo garantiza el índice).
--
-- Runtime inyecta :hub_id; :code lo pasa el `reads` del command desde `payload.code`.
SELECT id, code
FROM pricing_price_list
WHERE hub_id = :hub_id AND code = :code
