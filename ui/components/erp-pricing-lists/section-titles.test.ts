// Every table on the price lists screen carries its section title (pricing#48).
//
// The screen stacks two tables: price lists and discount rules. Only the second one had a heading
// («Discount rules»), so the price lists looked like part of something else. The contract follows
// the neighbouring screen with stacked tables (Reservations › Availability): an <h3> above EACH
// table, from the module catalogue, with the table's own load error under its heading.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

const LIST = { id: 'l1', code: 'L1', name: 'Tarifa mayorista 1', currency: 'EUR', is_default: 0, segment: null };
const RULE = { id: 'r1', code: 'R1', name: 'Descuento 1', rule_type: 'percent', value: '10', priority: 100 };

const locale = (lang: 'en' | 'es') =>
  JSON.parse(readFileSync(join(__dirname, '..', '..', '..', 'locales', `${lang}.json`), 'utf8')) as {
    ui: Record<string, unknown>;
  };

let failLists = false;

beforeEach(() => {
  failLists = false;
  (window as unknown as { matchMedia: unknown }).matchMedia = (query: string) => ({
    media: query,
    matches: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  });
  (globalThis as Record<string, unknown>).erplora = {
    query: async () => [],
    queryPage: async (name: string) => {
      if (name === 'pricing.price_lists.list') {
        if (failLists) throw new Error('lists_down');
        return { rows: [LIST], total: 1 };
      }
      return { rows: [RULE], total: 1 };
    },
    command: async () => ({}),
    on: () => () => {},
    formatMoney: (cents: number) => `${((cents || 0) / 100).toFixed(2)} €`,
    locale: 'es',
    t: (_catalog: unknown, key: string) => key,
  };
});

afterEach(() => {
  document.body.innerHTML = '';
});

type El = HTMLElement & { shadowRoot: ShadowRoot; updateComplete: Promise<unknown> };

async function mount(): Promise<El> {
  await import('./erp-pricing-lists');
  const el = document.createElement('erp-pricing-lists') as El;
  document.body.appendChild(el);
  await el.updateComplete;
  await new Promise((r) => setTimeout(r, 0));
  await el.updateComplete;
  return el;
}

/** The heading that labels a table: the closest <h3> above it, skipping only its own load error. */
function headingOf(table: Element): Element | null {
  let prev = table.previousElementSibling;
  while (prev && prev.matches('p.err')) prev = prev.previousElementSibling;
  return prev?.tagName === 'H3' ? prev : null;
}

describe('pricing#48: each table of the price lists screen has its section title', () => {
  it('the price lists table is headed by the «price lists» title', async () => {
    const el = await mount();
    const table = el.shadowRoot.querySelector('ok-data-table[testid="pricing-table"]')!;
    expect(headingOf(table)?.textContent?.trim()).toBe('ui.listsTitle');
  });

  it('the discount rules table keeps its own title', async () => {
    const el = await mount();
    const table = el.shadowRoot.querySelector('ok-data-table[testid="pricing-rules-table"]')!;
    expect(headingOf(table)?.textContent?.trim()).toBe('ui.rulesTitle');
  });

  it('there is one title per table, in the order of the tables', async () => {
    const el = await mount();
    const headings = [...el.shadowRoot.querySelectorAll('h3')].map((h) => h.textContent?.trim());
    expect(headings).toEqual(['ui.listsTitle', 'ui.rulesTitle']);
    expect(el.shadowRoot.querySelectorAll('ok-data-table')).toHaveLength(2);
  });

  it('when the price lists fail to load, the error sits under their title, not above it', async () => {
    failLists = true;
    const el = await mount();
    const error = el.shadowRoot.querySelector('[data-testid="pricing-load-error"]')!;
    expect(error).not.toBeNull();
    expect(error.previousElementSibling?.tagName).toBe('H3');
    expect(error.previousElementSibling?.textContent?.trim()).toBe('ui.listsTitle');
    expect(error.nextElementSibling?.getAttribute('testid')).toBe('pricing-table');
  });

  it('the title is translated: en «Price lists», es «Listas de precios»', () => {
    expect(locale('en').ui.listsTitle).toBe('Price lists');
    expect(locale('es').ui.listsTitle).toBe('Listas de precios');
  });

  it('the two section titles are different in both languages', () => {
    for (const lang of ['en', 'es'] as const) {
      const ui = locale(lang).ui;
      expect(typeof ui.rulesTitle).toBe('string');
      expect(String(ui.rulesTitle).trim()).not.toBe('');
      expect(ui.listsTitle).not.toBe(ui.rulesTitle);
    }
  });

  it('the orphaned «title» key is gone: the heading reads «listsTitle», nothing reads «title»', () => {
    expect(locale('en').ui.title).toBeUndefined();
    expect(locale('es').ui.title).toBeUndefined();
  });
});
