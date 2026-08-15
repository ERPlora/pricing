# Pricing — Overview

## What this module does

Pricing centralises **what something costs and why**. It holds **price lists** — a tariff per
currency, per customer segment, valid between dates, with a price per product and per quantity
bracket — and **discount rules** evaluated by priority. On top of that it exposes two calculators:
the final price of a product, and the discount applicable to an amount, **allocated line by line**.

Its distinctive feature is that a price never travels alone: every answer carries its **tax basis**
(gross or net), where that basis came from, the currency and the number of decimals.

## What this module does NOT do

- **It does not decide the price of a sale.** The till reads the product catalogue for that. This
  module is the tariff engine, and consuming it is the caller's choice.
- **It does not compute taxes.** It says whether a price includes tax; the rates live in `taxes`.
- **It does not know your products.** A price list item points at a product with a plain text
  reference and no foreign key.
- **It does not convert currencies.** Two lists in different currencies are two different lists.
- **It does not convert between tax bases.** Mixing them is an error, not something it silently
  reconciles.

## Modules it connects to

**Depends on nothing**, and nothing depends on it. Its events are published for anybody who wants
them; no module declares a listener.

**Events it emits**

| Event | When |
|---|---|
| `pricing.price_list.created` | a price list is created |
| `pricing.price_item.added` | a product price is added to a list |
| `pricing.rule.created` | a discount rule is created |
| `pricing.rule.deactivated` | a rule is deactivated |

**Events it listens to** — none.

## The two calculators

**Final price of a product** — pick the candidate lists (a given one, or all the active lists
matching the customer's segment plus the universal ones), match the quantity bracket, and return the
**lowest** price found, together with its currency, precision, tax basis and where that basis came
from.

**Discount on an amount** — resolve the applicable rules, sort them by priority, apply them one after
another to a running total, and return the original amount, the final amount, the total discount and
which rules fired. If you pass the **lines** of the order, it also returns the discount **allocated
per line** and aggregated **per tax rate**.

## Where its numbers come from

- **Money is integer cents** and the only rounding is HALF_UP (ADR-0123) — the same arithmetic the
  rest of the hub uses, not a private implementation.
- **An allocation is not a rounding.** Splitting one discount across N lines uses **largest
  remainder**, which preserves the total exactly.
- **Currency** defaults to `EUR`; each list declares its own.
- **Dates** are ISO `YYYY-MM-DD`.
