# Pricing — Screens

The module contributes one tab to the hub navigation: **Price Lists**.

## Price Lists

Every active price list of the hub, **with its tax basis** (`pricing.price_lists.list`, 50 rows per
page). Requires `pricing.view_pricing`. Sorted by name.

- **Search** by code or name.
- **Sort and filter** by code, name, currency, default flag, active flag, validity dates, segment or
  tax basis.

### Create a price list

1. Give it a **code** and a **name** — both required. The code is unique in the hub.
2. Set the **currency** (`EUR` by default).
3. Choose the **segment** it applies to: `customer`, `business`, `wholesale`, `retail`, or leave it
   empty for everyone.
4. Set **valid from** and **valid until** if it is seasonal.
5. Choose the **tax basis**:
   - **included** — prices carry the tax inside (a menu, a shop shelf);
   - **excluded** — prices are the taxable base and tax is added on top (B2B);
   - **inherit** — follow the hub's setting. This is the default and, for most hubs, the right
     answer.
6. Mark it **default** if it is the fallback tariff.

Creating a list marked default **demotes the previous default** in the same transaction — there is
only ever one. Requires `pricing.manage_pricing`.

### Add a product price to a list

1. Open the list and add an item.
2. Give the **product reference** and the **price**.
3. Optionally set the quantity bracket: a **minimum quantity** (1 by default) and a **maximum**.

The same product can appear several times in one list with different brackets — that is how volume
pricing is expressed.

The parent list must exist, belong to this hub and be **active**. If it does not, the whole thing is
**rolled back** and you get `pricing.price_list_unavailable`: nothing is written and no event is
emitted. Requires `pricing.manage_pricing`.

### See a list's prices

`pricing.price_lists.items` gives the products and prices of one list, ordered by product and
minimum quantity. `pricing.price_lists.items_by_product` gives the price of **one product across all
lists** — which is what the price engine needs, because it chooses between candidates.

## Discount rules

Rules evaluated by priority (`pricing.rules.list`, 50 rows per page). Requires
`pricing.view_pricing`.

- **Search** by code or name.
- **Sort and filter** by code, name, type, value, amount bounds, scope, conditions, validity, active
  flag or priority.

### Create a rule

1. Give it a **code**, a **name** and a **type** — all three required.
2. The type is one of:

   | Type | Meaning |
   |---|---|
   | `percent` | A percentage off |
   | `fixed` | A fixed amount off |
   | `tiered` | Different values by bracket |
   | `buy_x_get_y` | Quantity promotion |

3. Set the **value**, and optionally a **minimum** and **maximum amount** for it to apply.
4. Choose the scope: `all`, `customer_segment` or `product_category`.
5. Add extra **conditions** as free-form data if the rule needs them.
6. Set the **validity dates** and the **priority** — **a lower number applies earlier**.

Requires `pricing.manage_pricing`.

### Retire a rule

Deactivate it. The rule stops applying but is **not deleted** — this is not a soft delete, just a
flag. Requires `pricing.manage_pricing`.

## Asking for a price

Give a product reference, a quantity and optionally a customer segment or a specific list. You get
back the price, which list it came from, the currency, the number of decimals, and **the tax basis
with its origin** (`price_list`, `hub` or `default`).

If the candidate lists disagree about their tax basis, the request **fails** with
`mixed_tax_basis` — comparing a gross price with a net one is meaningless and this module refuses to
do it silently.

Requires `pricing.apply_pricing` — an employee has this.

## Asking for a discount

Give an amount, and optionally the rules to consider (all active ones, specific ids, or rules
supplied inline) and the **lines** of the order.

You get back the original amount, the final amount, the total discount, which rules fired, and the
tax basis. **With lines**, you also get:

- **the allocation** — how much discount each line takes, and what it costs afterwards;
- **the aggregate per tax rate** — the base before, the discount and the base after, per rate.

That per-rate aggregate is exactly what an invoice needs to build its breakdown.

Requires `pricing.apply_pricing`.

> Both calculators are read-only, and today **their answer does not reach the caller**: the host
> returns success and an operation count. The contract is written, compiled and tested; the channel
> that hands a read-only result back is a runtime feature that does not exist yet.
