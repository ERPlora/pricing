-- Every item of ONE product, across all the hub's lists (scoped by :hub_id).
--
-- This is the read `get_price` needs: the engine chooses among candidate lists (by explicit id or
-- by customer segment), so it needs the product's price in ALL of them, not in one.
-- `pricing.price_lists.items` (by :price_list_id) stays for the UI of a single list; this one is
-- what the command declares as `reads`.
SELECT id, price_list_id, product_ref, price, min_quantity, max_quantity
FROM pricing_price_list_item
WHERE product_ref = :product_ref AND hub_id = :hub_id AND is_deleted = 0
ORDER BY price_list_id ASC, min_quantity ASC;
