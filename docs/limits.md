# Pricing — Limits and troubleshooting

## Known limitations you should know about

- **The calculators' answers do not reach the caller.** `get_price` and `calculate_discount` are
  read-only and produce a full result, but the host returns only success and an operation count.
  There is no channel yet for a read-only handler result. The contract is written, compiled and
  tested; it will be picked up **without any manifest change** when the runtime supports it.
- **No currency conversion.** Two currencies are two separate lists.
- **No conversion between tax bases.** Mixing them is refused.
- **No screen for rules.** The navigation entry is the price lists; rules are reachable through the
  API. <!-- TODO: verify -->

## Errors you will actually see

| Error | What happened | What to do |
|---|---|---|
| `pricing.price_list_unavailable` | The price list you added an item to does not exist, is not yours, or is inactive | Check the list; nothing was written and no event was emitted |
| `no_price_list` | No candidate list matched | Create a list, or check the segment and the validity dates |
| `no_price` | The product has no price in the candidate lists for that quantity | Add the item, or check the quantity bracket |
| `invalid_quantity` | The quantity is not usable | Give a positive quantity |
| `invalid_id` | A malformed identifier | Check the id |
| **`mixed_tax_basis`** | The candidate lists disagree about gross vs net | Do not mix bases; ask for a specific list, or fix the lists |
| `invalid_amount` | The amount is not usable | Give a valid amount in cents |
| **`amount_mismatch`** | The lines you sent do not add up to the amount you sent | Fix the input; the module refuses to allocate against an inconsistent total |

## Required fields

| Action | Must provide |
|---|---|
| Create a price list | `code`, `name` |
| Add a price item | `price_list_id`, `product_ref`, `price` |
| Create a rule | `code`, `name`, `rule_type` |
| Deactivate a rule | `rule_id` |
| Get a price | `product_ref` |
| Calculate a discount | nothing is strictly required, but an amount is what makes it useful |

## Accepted values

| Field | Values |
|---|---|
| Rule type | `percent`, `fixed`, `buy_x_get_y`, `tiered` |
| Rule scope | `all`, `customer_segment`, `product_category` |
| List segment | `customer`, `business`, `wholesale`, `retail`, or empty for any |
| Tax basis | included, excluded, or inherit (empty) |
| Currency | default `EUR` |
| Dates | ISO `YYYY-MM-DD` |
| Priority | lower applies first |
| Minimum quantity | default 1; maximum optional |

## Caps and sizes

| Limit | Value |
|---|---|
| Rows per page (price lists, rules) | 50 |
| Maximum rows a paginated request may ask for | 500 |
| Default price lists per hub | 1 |
| Price list codes | unique per hub |
| Rule codes | unique per hub |

## Permissions per action

| To do this | You need |
|---|---|
| See price lists, their items and the rules | `pricing.view_pricing` |
| Create a list, add an item, create or deactivate a rule | `pricing.manage_pricing` |
| Ask for a price or a discount | `pricing.apply_pricing` |

By role: **admin** has everything. **manager** has all three. **employee** can **see and apply**
prices but **cannot manage** them — no creating lists, no adding prices, no touching rules.

## Dependencies

**None in either direction.** Pricing depends on no module and no module declares it as a dependency.

The practical consequence is important: **installing Pricing does not change what the till charges.**
The sale price of a catalogue line comes from `inventory`. This module is a tariff engine that a
caller must choose to consult; it does not intercept anything.

The coupling with the catalogue is by convention only — a product reference is a string, with no
foreign key.

## When something looks wrong

**"The engine says there is no price list."** No candidate matched. Check that a list is **active**,
that its validity dates cover today, and that its segment matches (or is empty).

**"It picked the wrong list."** It returns the **cheapest** matching price across candidates. If you
need a specific tariff regardless of price, request that list explicitly.

**"`mixed_tax_basis` on a simple lookup."** Two candidate lists disagree about gross versus net.
Either restrict the query to one list, or make the lists agree — usually by setting them to inherit.

**"The price looks 21 % off."** Check the tax basis of the list. A net list returns the taxable base;
tax is added on top by whoever builds the document.

**"I changed the hub's tax mode and old documents changed."** They must not. A document freezes its
basis on the line. If yours changed, the snapshot is not being respected.

**"Discounts do not add up to the total."** They do, by construction — the allocation uses largest
remainder precisely so the sum is exact. If a figure is off, check whether each share is being
rounded separately somewhere downstream; that is the classic mistake.

**"A line went negative after a big discount."** It cannot: the discount is capped at the order
total and no line goes below zero.

**"Two identical orders produced different discounts."** They cannot; the allocation is
deterministic. Check whether the rules changed between the two runs.

**"Rules are compounding unexpectedly."** They are meant to: each applies to what the previous left.
Use the priority to control the order — **lower first**.

**"I deactivated a rule and it is still in the list."** Deactivation is a flag, not a delete.

**"Adding a price silently did nothing."** It did not fail silently — it returned
`pricing.price_list_unavailable` and rolled everything back. The list is missing, foreign or
inactive.
