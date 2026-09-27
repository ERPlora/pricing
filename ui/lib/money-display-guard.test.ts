import { it, expect } from 'vitest';
import { checkMoneyDisplay } from '@erplora/module-toolkit/money-display-guard';

// GUARD (pm#289, shared since pm#505/pm#508): money on screen is never formatted by hand in this
// module, and OutfitKit comes in by entry point, never as a value from the barrel.
//
// The rules live in `@erplora/module-toolkit/money-display-guard` (one piece for every module,
// tested there against its own positives); this test only says what is specific to Precios:
//
// * witnesses — the one amount this module paints (the VALUE column of a `fixed` rule) goes
//   through the shell's formatter. They count the CALL, not the name: the screen also declares
//   `formatMoney(cents: number, …)` in its `erplora()` interface, and a scan over empty or
//   over-stripped content must not stay green on that declaration (rv-combos-22).
// * outfitkitImporters — the list screen is the one that imports OutfitKit (entry point + types),
//   so the barrel scan provably read it (rv-pricing-53).
// * notDisplay — none: pricing has no hand formatting that is not a screen amount. Add an entry
//   (`'file: exact code line'` → why) only with the reason it is not a screen amount.
it('money on screen goes through the shared formatter and OutfitKit by entry point (pm#289)', () => {
  expect(
    checkMoneyDisplay({
      from: import.meta.url,
      witnesses: {
        'components/erp-pricing-lists/erp-pricing-lists.ts': { text: 'erplora().formatMoney(', atLeast: 1 },
        'lib/rule-value.ts': { text: 'formatMoney(num(rule.amount_cents))', atLeast: 1 },
      },
      notDisplay: {},
      outfitkitImporters: ['components/erp-pricing-lists/erp-pricing-lists.ts'],
    }),
  ).toEqual([]);
});
