-- Active price lists of the hub. The runtime injects :hub_id.
--
-- `tax_included` (ADR-0210) always travels with the row: it is what makes a price mean something.
-- Tri-state (1 gross · 0 taxable base · NULL = inherit the hub) — the handler resolves it; the
-- query does not interpret it.
SELECT id, code, name, currency, is_default, is_active, valid_from, valid_until, segment,
       tax_included
FROM pricing_price_list
WHERE hub_id = :hub_id AND is_deleted = 0 AND is_active = 1
