-- One price list by id (scoped by hub_id).
-- `tax_included` (ADR-0210): tri-state 1 gross · 0 taxable base · NULL = inherit the hub.
SELECT id, code, name, currency, is_default, is_active, valid_from, valid_until, segment,
       tax_included
FROM pricing_price_list
WHERE id = :price_list_id AND hub_id = :hub_id AND is_deleted = 0;
