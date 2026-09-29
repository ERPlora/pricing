// pm#513 (out of pm#478) — on a phone or a tablet, a refused «New price list» showed NOTHING: the
// person pressed «Save» and the sheet stayed as it was.
//
// The refusal did arrive; it was painted in the wrong place. The form lives in the `create` panel of
// the lists `ok-data-table`, and under 834 px that panel is a FULL-SCREEN sheet (outfitkit#75). A
// refusal that is not about one field (anything but a repeated code) was a child of the PAGE, so on a
// phone it sat under the sheet, out of sight (bench: hub:stable 1.1.30, 390 and 820 px, ios and md:
// 4 of 6 hidden). A repeated code already hung from the «Code» field and took the focus (pricing#46),
// so that one was always seen.
//
// The rule, the same one customers#97 / tables#93 / reservations#73 / appointments#227 / tasks#47 /
// tickets#43 / cart_checkout#31 follow:
//
//   · what goes wrong while SAVING the form is painted INSIDE that form, above the button that was
//     pressed, and scrolled into view once — not again on every keystroke (rv-reservations-73);
//   · what goes wrong OUTSIDE the save stays on the PAGE: a list of price lists or of discount rules
//     that does not load. No panel is open then, and a notice inside a closed panel is just as
//     invisible (rv-appointments-227).
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { dataTableShowsLoadError } from '@erplora/module-sdk';
import en from '../../../locales/en.json';

/** The real `en` catalog: the view shows what the person reads, not a key. */
const translate = (key: string): string =>
  (key.split('.').reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], en) as string) ?? key;

const LIST = { id: 'l1', code: 'BASE', name: 'Base', currency: 'EUR', is_default: 1, segment: null, tax_included: null };
const RULE = { id: 'r1', code: 'BF', name: 'Black Friday', rule_type: 'percent', value: '10', amount_cents: '0', priority: 1 };

const LOAD_FAILURE = 'The server is not answering.';

/** Code of the refusal the next command throws, or null for a save that goes through. */
let refuse: string | null = null;
/** When set, the next command waits on it: lets a test look at the screen while a save is in flight. */
let hold: Promise<void> | null = null;
/** Which list query fails to load, if any. */
let loadFails: string | null = null;
/** How many times each list was read: a save that goes through reloads the lists. */
let reads: Record<string, number> = {};
let commands: string[] = [];
/** Every element the component scrolled into view. */
let revealed: Element[] = [];

beforeEach(() => {
  refuse = null;
  hold = null;
  loadFails = null;
  reads = {};
  commands = [];
  revealed = [];
  vi.spyOn(HTMLElement.prototype, 'scrollIntoView').mockImplementation(function (this: HTMLElement) {
    revealed.push(this);
  });
  (globalThis as Record<string, unknown>).erplora = {
    query: async () => [],
    queryPage: async (name: string) => {
      reads[name] = (reads[name] ?? 0) + 1;
      if (loadFails === name) throw new Error(LOAD_FAILURE);
      return { rows: name === 'pricing.price_lists.list' ? [LIST] : [RULE], total: 1 };
    },
    command: async (name: string) => {
      commands.push(name);
      const wait = hold;
      if (wait) await wait;
      if (refuse) throw Object.assign(new Error('refused'), { code: refuse });
      return {};
    },
    on: () => () => {},
    locale: 'es',
    t: (_c: unknown, key: string) => translate(key),
    formatMoney: (cents: number) => `${(cents / 100).toFixed(2)} €`,
  };
});

type Wc = HTMLElement & { shadowRoot: ShadowRoot; updateComplete: Promise<unknown> } & Record<string, any>;

async function mount(): Promise<Wc> {
  await import('./erp-pricing-lists');
  const el = document.createElement('erp-pricing-lists') as Wc;
  document.body.appendChild(el);
  await settle(el);
  return el;
}

async function settle(el: Wc): Promise<void> {
  for (let i = 0; i < 3; i++) {
    await el.updateComplete;
    await new Promise((r) => setTimeout(r, 0));
  }
}

const submitEvent = (): Event => new Event('submit', { cancelable: true });

const CREATE = 'form[slot="create"]';
/** The form-wide refusal: the module's generic message. */
const GENERIC = translate('ui.createListError');

/** The notice inside `scope`, or null. */
const inside = (el: Wc, scope: string, testid: string): Element | null =>
  el.shadowRoot.querySelector(`${scope} [data-testid="${testid}"]`);

/** The reason a list did not load, as the PAGE shows it: on the list's own table when the shell's
 *  table paints a failed load itself (pm#533) — and then with no page notice, which would say it
 *  twice —, in the page notice on an older shell. Null when the page does not show it. */
function loadFailureOnPage(el: Wc, table: string, notice: string): string | null {
  const pageNotice = inside(el, '.page', notice);
  if (!dataTableShowsLoadError()) return pageNotice?.textContent?.trim() || null;
  const t = el.shadowRoot.querySelector<HTMLElement & { error?: string }>(`.page ok-data-table[testid="${table}"]`);
  return pageNotice ? null : t?.error || null;
}

/** Every place a notice with `text` is painted in, by where it sits. */
function whereIs(el: Wc, text: string): string[] {
  return [...el.shadowRoot.querySelectorAll('.err')]
    .filter((n) => n.textContent?.trim() === text)
    .map((n) => (n.closest(CREATE) ? 'panel' : 'page'));
}

/** The notice sits above the button of its form. */
function aboveTheButton(form: Element, testid: string): boolean {
  const kids = [...form.children];
  const notice = kids.findIndex((k) => k.getAttribute('data-testid') === testid);
  const button = kids.findIndex((k) => k.tagName === 'ION-BUTTON' && k.getAttribute('type') === 'submit');
  return notice >= 0 && button >= 0 && notice < button;
}

async function refusedCreate(el: Wc, code = 'pricing.bench_refusal'): Promise<void> {
  el.newCode = 'OFERTAS';
  el.newName = 'Ofertas';
  refuse = code;
  await el.createList(submitEvent());
  await settle(el);
}

describe('pm#513 · price lists: a refused «Save» is shown INSIDE the panel form', () => {
  it('lands in the form, with its text, and is scrolled into view — nothing on the page under the sheet', async () => {
    const el = await mount();
    await refusedCreate(el);
    const notice = inside(el, CREATE, 'pricing-form-error');
    expect(notice, 'on a phone the panel covers the page: the refusal has to travel with the form').not.toBeNull();
    expect(notice?.textContent?.trim()).toBe(GENERIC);
    expect(revealed, 'and it is scrolled into view').toEqual([notice]);
    expect(whereIs(el, GENERIC)).toEqual(['panel']);
  });

  it('sits above the «Save» button that was pressed', async () => {
    const el = await mount();
    await refusedCreate(el);
    expect(aboveTheButton(el.shadowRoot.querySelector(CREATE)!, 'pricing-form-error')).toBe(true);
  });

  it('is revealed once, not again on every keystroke while the person corrects the form', async () => {
    const el = await mount();
    await refusedCreate(el);
    revealed = [];
    el.newName = 'Ofertas de verano';
    await settle(el);
    el.newCurrency = 'USD';
    await settle(el);
    expect(inside(el, CREATE, 'pricing-form-error'), 'the refusal is still there').not.toBeNull();
    expect(revealed, 'but the sheet stays where the person is typing').toEqual([]);
  });

  it('a second refusal is revealed again', async () => {
    const el = await mount();
    await refusedCreate(el);
    revealed = [];
    await refusedCreate(el, 'pricing.another_refusal');
    expect(revealed).toEqual([inside(el, CREATE, 'pricing-form-error')]);
  });

  it('while the new attempt is being saved, the previous refusal is already gone', async () => {
    const el = await mount();
    await refusedCreate(el);
    refuse = null;
    let release!: () => void;
    hold = new Promise((r) => (release = r));
    const attempt = el.createList(submitEvent());
    await settle(el);
    expect(whereIs(el, GENERIC)).toEqual([]);
    release();
    await attempt;
  });

  it('a repeated code still hangs from the «Code» field, not from the form (pricing#29/#46)', async () => {
    const el = await mount();
    await refusedCreate(el, 'pricing.duplicate_code');
    const onField = el.shadowRoot.querySelector('[data-testid="pricing-code"]')?.getAttribute('error-text') ?? '';
    expect(onField, 'the message hangs from «Code»').toBe(translate('ui.errDuplicateCode'));
    expect(inside(el, CREATE, 'pricing-form-error'), 'the message is said once, on its field').toBeNull();
    expect(whereIs(el, onField)).toEqual([]);
  });

  it('a save that goes through reloads the lists table', async () => {
    const el = await mount();
    const before = reads['pricing.price_lists.list'];
    el.newCode = 'OFERTAS';
    el.newName = 'Ofertas';
    await el.createList(submitEvent());
    await settle(el);
    expect(commands).toEqual(['pricing.price_lists.create']);
    expect(reads['pricing.price_lists.list'], 'the new list would not show until the next visit').toBe(before + 1);
  });
});

describe('pm#513 · price lists: what goes wrong OUTSIDE the save stays on the page (rv-appointments-227)', () => {
  it('a list of price lists that does not load is shown on the page, not in the form', async () => {
    loadFails = 'pricing.price_lists.list';
    const el = await mount();
    expect(loadFailureOnPage(el, 'pricing-table', 'pricing-load-error')).toBe(LOAD_FAILURE);
    expect(inside(el, CREATE, 'pricing-load-error'), 'a closed panel hides it').toBeNull();
    expect(inside(el, CREATE, 'pricing-form-error')).toBeNull();
  });

  it('a list of discount rules that does not load is shown on the page, not in the form', async () => {
    loadFails = 'pricing.rules.list';
    const el = await mount();
    expect(loadFailureOnPage(el, 'pricing-rules-table', 'pricing-rules-load-error')).toBe(LOAD_FAILURE);
    expect(inside(el, CREATE, 'pricing-rules-load-error'), 'a closed panel hides it').toBeNull();
    expect(inside(el, CREATE, 'pricing-form-error')).toBeNull();
  });

  it('with a load failure on the page, a refused save brings the FORM notice into view, not the page one', async () => {
    // The lists load failure is painted ABOVE the form: the first notice of the page is not the refusal.
    loadFails = 'pricing.price_lists.list';
    const el = await mount();
    await refusedCreate(el);
    expect(revealed).toEqual([inside(el, CREATE, 'pricing-form-error')]);
    expect(loadFailureOnPage(el, 'pricing-table', 'pricing-load-error'), 'the load failure is still on the page').toBe(LOAD_FAILURE);
  });

  it('a load failure is not scrolled to as if it were a refused save', async () => {
    loadFails = 'pricing.price_lists.list';
    await mount();
    expect(revealed).toEqual([]);
  });
});
