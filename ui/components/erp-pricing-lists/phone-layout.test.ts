// The price lists screen on a phone (pricing#47).
//
// Measured on a hub at 390 px (ios and md): the screen stacks TWO tables (price lists and discount
// rules) and both filled the height (`fill`), so they split it in half and each one scrolled its
// cards inside a 108 px slot — not even one whole card showed, the rest hid behind the pinned
// «N records» footer.
//
// The contract, the way Shopify, Square and Odoo lay lists out on a phone: when the tables open as
// cards (ok-data-table switches at 640 px), neither fills the screen; each keeps its full height,
// its footer follows its last card, and the page scrolls as one. A tablet or desktop keeps `fill`:
// there each table shows a list of rows with a fixed pager, which is right.
//
// The name repeated inside every card was ok-data-table's (outfitkit#205), not this module's.
//
// happy-dom does not lay out: this pins the contract; the bench screenshots in the PR prove it.
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

const LIST = { id: 'l1', code: 'L1', name: 'Tarifa mayorista 1', currency: 'EUR', is_default: 0, segment: null };
const RULE = { id: 'r1', code: 'R1', name: 'Descuento 1', rule_type: 'percent', value: '10', priority: 100 };

// A controllable `(max-width: 640px)`: the width at which ok-data-table opens as cards.
let phone = false;
let listeners: { query: string; l: (e: { matches: boolean }) => void }[] = [];
const realMatchMedia = window.matchMedia;

function setPhone(on: boolean): void {
  phone = on;
  for (const { l } of listeners) l({ matches: on });
}

beforeEach(() => {
  phone = false;
  listeners = [];
  (window as unknown as { matchMedia: unknown }).matchMedia = (query: string) => ({
    media: query,
    get matches() {
      return query === '(max-width: 640px)' ? phone : false;
    },
    addEventListener: (_: string, l: (e: { matches: boolean }) => void) => listeners.push({ query, l }),
    removeEventListener: (_: string, l: (e: { matches: boolean }) => void) => {
      listeners = listeners.filter((x) => x.l !== l);
    },
  });
  (globalThis as Record<string, unknown>).erplora = {
    query: async () => [],
    queryPage: async (name: string) => ({ rows: name === 'pricing.price_lists.list' ? [LIST] : [RULE], total: 1 }),
    command: async () => ({}),
    on: () => () => {},
    formatMoney: (cents: number) => `${((cents || 0) / 100).toFixed(2)} €`,
    locale: 'es',
    t: (_catalog: unknown, key: string) => key,
  };
});

afterEach(() => {
  document.body.innerHTML = '';
  window.matchMedia = realMatchMedia;
});

type El = HTMLElement & { shadowRoot: ShadowRoot; updateComplete: Promise<unknown> };

async function settle(el: El) {
  await el.updateComplete;
  await new Promise((r) => setTimeout(r, 0));
  await el.updateComplete;
}

async function mount(): Promise<El> {
  await import('./erp-pricing-lists');
  const el = document.createElement('erp-pricing-lists') as El;
  document.body.appendChild(el);
  await settle(el);
  return el;
}

const fills = (el: El) => [...el.shadowRoot.querySelectorAll('ok-data-table')].map((t) => (t as HTMLElement & { fill: boolean }).fill);

function styles(): string {
  const ctor = customElements.get('erp-pricing-lists') as unknown as { styles: unknown };
  const sheets = Array.isArray(ctor.styles) ? ctor.styles : [ctor.styles];
  return sheets.map((s) => String((s as { cssText?: string })?.cssText ?? s)).join('\n').replace(/\s+/g, ' ');
}

describe('on a phone the two lists scroll as one page, each footer after its last card (pricing#47)', () => {
  it('neither table fills the screen on a phone, so they do not split it into two slots', async () => {
    phone = true;
    const el = await mount();
    expect(fills(el), 'fill on a phone: each table scrolls its cards inside half the screen').toEqual([false, false]);
  });

  it('a tablet or desktop keeps both tables filling the height, with a fixed pager', async () => {
    const el = await mount();
    expect(fills(el)).toEqual([true, true]);
  });

  it('follows the width live: turning the device or resizing the window switches the layout', async () => {
    const el = await mount();
    setPhone(true);
    await settle(el);
    expect(fills(el)).toEqual([false, false]);
    setPhone(false);
    await settle(el);
    expect(fills(el)).toEqual([true, true]);
  });

  it('stops listening to the width once the screen is gone', async () => {
    // ok-data-table listens to the same query for its own card switch: count them all, none may stay.
    const el = await mount();
    expect(listeners.length).toBeGreaterThan(0);
    el.remove();
    expect(listeners.map((x) => x.query)).toEqual([]);
  });

  it('the page is the one that scrolls, and a table that does not fill keeps its own height', () => {
    const css = styles();
    expect(css, 'without its own scroll the page cannot show the cards below the first screen').toMatch(/\.page \{[^}]*overflow-y:\s*auto/);
    expect(css, 'flex:1 1 0 squashes a non-filling table back into a slot').toMatch(/\.page > ok-data-table:not\(\[fill\]\) \{[^}]*flex:\s*0 0 auto/);
  });
});
