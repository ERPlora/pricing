# Pricing — Concepts

The things people get wrong on their first day.

## A price is not a number — it comes with its tax basis

Every price list declares whether its prices are **gross** (tax included) or **net** (the taxable
base, tax added on top), and every answer this module gives carries that basis **plus where it came
from**: the list itself, the hub's setting, or the built-in default.

Why it matters: `1000` means 10,00 € to the customer in one list and 10,00 € **plus tax** in another.
A number without its basis is not a price, it is a rumour.

## The tax basis has three states, and "inherit" is the useful one

| Value | Meaning |
|---|---|
| **included** | Prices carry the tax inside — a menu, a shop shelf |
| **excluded** | Prices are the taxable base — B2B |
| **inherit** (empty) | Follow the hub's setting; if the hub is silent, gross |

Existing lists were deliberately left on **inherit**. Filling them in with today's hub setting would
have frozen that moment into every list forever and unhooked them from the hub.

## Mixing bases is an error, not a comparison

If the engine has to choose between candidate lists that disagree about their basis, it **refuses**
with `mixed_tax_basis` rather than picking the "cheapest" number.

The same applies downstream: **a single document must not mix bases.** If a conversion is genuinely
needed, whoever needs it converts explicitly and documents it — this module will not do it behind
your back.

## The engine returns the lowest matching price

Given the candidate lists (a specific one, or all the active lists matching the customer's segment
plus the universal ones), it matches the quantity bracket and returns the **cheapest** price found.

So a customer in a segment with a worse price than the universal list gets the universal one. That is
the intended behaviour; if you need "the segment list always wins", ask for that list explicitly.

## Quantity brackets are how volume pricing works

A price list item has a minimum quantity (one unit by default) and an optional maximum. The same
product appears several times in one list, once per bracket. A quantity is priced by the bracket it
falls in, and **both edges are inclusive**: "from 10 to 99" includes both the 10 and the 99.

### Quantities are integers of a millionth (ADR-0147)

Every quantity in the API — the two edges of a bracket and the quantity you ask a price for — is an
**integer in millionths of a unit**. One unit is `1000000`; half a kilo is `500000`. This is the same
scale the rest of the product speaks, so a quantity travels from the till to the kitchen to the stock
move without anyone rescaling it on the way.

Two consequences worth knowing:

- **A bare `1` is one millionth of a unit**, not one unit. A caller still speaking in whole units
  will find that no bracket matches, which is deliberate: a loud `no_price` beats a wholesale price
  charged for a single item.
- **Fractions are refused, not rounded.** `0.5` is not a valid quantity — `500000` is. Truncating it
  is how half a kilo used to become zero.

## Rules apply in priority order, one after another

Rules are sorted by **priority ascending — the lower number goes first** — and each one applies to the
**running total** left by the previous. They compound; they are not all computed against the original
amount.

The result is clamped to the range `[0, running total]`, so a discount can never make an order
negative and can never exceed what is left.

## Deactivating a rule is not deleting it

It sets a flag. The rule stops applying to new calculations and stays in the list. There is no
delete.

## Allocating a discount is not rounding it

This is the subtlest thing in the module and it is worth understanding.

Rounding produces **one** amount from a fraction, and the hub does that HALF_UP. **Splitting one
amount across N lines is a different operation**, and rounding each share separately does not
preserve the total: 101 cents shared between three equal lines would give 34 + 34 + 34 = **102**.

So the allocation uses **largest remainder**: each line takes the **floor** of its proportional
share, and the leftover cents are handed out one at a time to the lines with the biggest fractional
part, ties broken by input order.

The guarantees that follow:

- the allocated discounts **sum exactly** to the total discount, and so do the per-rate figures;
- the per-rate amounts after discount sum exactly to the final amount;
- **no line ever goes below zero** — a discount bigger than the order is capped at the order;
- **the sign travels**: a surcharge allocated stays a surcharge on every line;
- an order that is entirely zero allocates zero, without dividing by zero;
- it is **deterministic** — the same order billed twice produces the same figures.

## The order of calculation is fixed

Whoever consumes this module must follow it:

**price → discount (allocated) → base and tax per rate → rounding, once per rate.**

Rounding earlier, or rounding a total instead of each rate, produces a document that does not add up.

## The tax basis must be frozen on the line

When a sale or an invoice uses a price, it **snapshots** the basis along with the rest of its fiscal
data. A refund or a cancellation reproduces the original document from that snapshot — **never** from
the hub's current setting.

Changing the hub's tax mode must not rewrite what was already charged.

## Only one default price list per hub

Creating a list marked default demotes the previous one **atomically, in the same transaction**. This
is not enforced by a database index; it is guaranteed by that flip.

## Product references are plain strings

A price list item points at a product by a text reference, with **no foreign key** to any catalogue.
Renaming or deleting a product elsewhere does not touch the price list.

## Money is cents, and the arithmetic is shared

All amounts are integer cents, and the arithmetic and the single HALF_UP rounding come from the hub's
shared money implementation (ADR-0123) — not from a copy inside this module. That is deliberate: a
divergent rounding here would mean charging one thing and declaring another.
