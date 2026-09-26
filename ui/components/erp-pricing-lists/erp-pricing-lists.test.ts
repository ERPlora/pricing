// Contrato de la BARRA de la vista de tarifas (listas de precios + reglas de descuento).
//
// El alta de una lista de precios vive DENTRO de su `ok-data-table` —la PRIMERA, la de listas—,
// detrás del «+» de su barra (panel `slot="create"`), como en /employees del core y en el CRUD de
// productos de `inventory`; no en un `<form>` suelto encima de las tablas. Tras crear con éxito el
// panel se cierra solo. La tabla de reglas de descuento NO tiene alta en esta vista: no declara
// «+» (un botón muerto es peor que ninguno).
//
// Los filtros van dentro de la tabla y los de dominio cerrado se eligen con un `select`, con las
// opciones que declara la migración: `segment` ∈ customer|business|wholesale|retail y
// `rule_type` ∈ percent|fixed|buy_x_get_y|tiered.
import { beforeEach, describe, expect, it } from 'vitest';

const LISTA = { id: 'l1', code: 'BASE', name: 'Tarifa base', currency: 'EUR', is_default: 1, segment: 'retail' };
const REGLA = { id: 'r1', code: 'BLACK', name: 'Black Friday', rule_type: 'percent', value: '10', priority: 1 };

const comandos: { name: string; payload: Record<string, unknown> }[] = [];

beforeEach(() => {
  comandos.length = 0;
  (globalThis as Record<string, unknown>).erplora = {
    query: async () => [],
    queryPage: async (name: string) => ({
      rows: name === 'pricing.price_lists.list' ? [LISTA] : [REGLA],
      total: 1,
    }),
    command: async (name: string, payload: Record<string, unknown>) => {
      comandos.push({ name, payload });
      return {};
    },
    on: () => () => {},
    // Contrato REAL del SDK: recibe CÉNTIMOS y divide (ADR-0007).
    formatMoney: (cents: number) => `${((cents || 0) / 100).toFixed(2)} €`,
    locale: 'es',
    t: (_catalog: unknown, key: string) => key,
  };
});

async function montar() {
  await import('./erp-pricing-lists');
  const el = document.createElement('erp-pricing-lists');
  document.body.appendChild(el);
  await (el as unknown as { updateComplete: Promise<unknown> }).updateComplete;
  await new Promise((r) => setTimeout(r, 0));
  await (el as unknown as { updateComplete: Promise<unknown> }).updateComplete;
  return el as HTMLElement & { shadowRoot: ShadowRoot };
}

const tablas = (el: HTMLElement & { shadowRoot: ShadowRoot }) =>
  [...el.shadowRoot.querySelectorAll('ok-data-table')] as (HTMLElement & { addable: boolean; fill: boolean; close: () => void })[];

describe('el alta vive DENTRO de la tabla de listas (paridad con /employees e inventory)', () => {
  it('la tabla de listas declara `addable` → pinta el «+»; la de reglas NO (no tiene alta)', async () => {
    const el = await montar();
    const [listas, reglas] = tablas(el);
    expect(listas?.addable, 'sin `addable` no hay «+» en la barra de la tabla de listas').toBe(true);
    expect(reglas?.addable, 'la tabla de reglas pinta un «+» que no da de alta nada').toBe(false);
  });

  it('las tablas llenan el alto del contenedor (`fill`): scroll interno, pager fijo', async () => {
    const el = await montar();
    expect(tablas(el).map((t) => t.fill)).toEqual([true, true]);
  });

  it('el formulario de alta se proyecta en el panel `create` de la tabla de listas', async () => {
    const el = await montar();
    const form = el.shadowRoot.querySelector('form[slot="create"]');
    expect(form, 'el formulario de alta no está en el slot `create`').toBeTruthy();
    expect(form?.closest('ok-data-table'), 'el alta de lista no cuelga de la tabla de listas').toBe(tablas(el)[0]);
  });

  it('no queda NINGÚN control de alta suelto fuera de las tablas', async () => {
    const el = await montar();
    const sueltos = [...el.shadowRoot.querySelectorAll('form, ion-input, ion-select, ion-button')].filter(
      (n) => !n.closest('ok-data-table'),
    );
    expect(sueltos.map((n) => n.tagName.toLowerCase()), 'hay controles de alta fuera de las tablas').toEqual([]);
  });
});

describe('los filtros de dominio cerrado son `select`', () => {
  it('el segmento de la lista se elige, no se teclea', async () => {
    const el = await montar();
    const cols = (el as unknown as { listColumns: { key: string; filterType?: string; options?: { value: string }[] }[] }).listColumns;
    const segmento = cols.find((c) => c.key === 'segment');
    expect(segmento?.filterType, 'el segmento se filtra con texto libre').toBe('select');
    expect(segmento?.options?.map((o) => o.value)).toEqual(['customer', 'business', 'wholesale', 'retail']);
  });

  it('el tipo de regla de descuento se elige del enum de la migración', async () => {
    const el = await montar();
    const cols = (el as unknown as { ruleColumns: { key: string; filterType?: string; options?: { value: string }[] }[] }).ruleColumns;
    const tipo = cols.find((c) => c.key === 'rule_type');
    expect(tipo?.filterType, 'el tipo de regla se filtra con texto libre').toBe('select');
    expect(tipo?.options?.map((o) => o.value)).toEqual(['percent', 'fixed', 'buy_x_get_y', 'tiered']);
  });
});

// ADR-0210: a price list says whether its prices already carry the tax. If the field is only in
// the DB, nobody can set it — and the whole point is that a B2B list and a retail list can coexist
// without anyone guessing. It is a CLOSED domain with THREE states, so it is chosen, never typed:
// included · excluded · inherit the hub (the default, stored as NULL).
describe('the tax basis of a price list is set from the UI', () => {
  it('the create panel offers the three states, and "inherit" is the default', async () => {
    const el = await montar();
    const select = el.shadowRoot.querySelector('form[slot="create"] ion-select');
    expect(select, 'there is no way to set the tax basis when creating a list').toBeTruthy();
    const options = [...(select?.querySelectorAll('ion-select-option') ?? [])].map((o) =>
      o.getAttribute('value'),
    );
    expect(options).toEqual(['', '1', '0']);
    expect(
      (el as unknown as { newTaxIncluded: string }).newTaxIncluded,
      'a new list must inherit the hub unless the user says otherwise',
    ).toBe('');
  });

  it('"inherit" is sent as NULL, not as a resolved value', async () => {
    const el = await montar();
    const wc = el as unknown as {
      newCode: string; newName: string; createList: (ev: Event) => Promise<void>;
    };
    wc.newCode = 'RETAIL';
    wc.newName = 'Retail';
    await wc.createList(new Event('submit'));
    const alta = comandos.find((c) => c.name === 'pricing.price_lists.create');
    expect(alta!.payload.tax_included, 'NULL means "follow the hub", not "assume something"').toBeNull();
  });

  it('a B2B list is created with the tax EXCLUDED', async () => {
    const el = await montar();
    const wc = el as unknown as {
      newCode: string; newName: string; newTaxIncluded: string; createList: (ev: Event) => Promise<void>;
    };
    wc.newCode = 'B2B';
    wc.newName = 'Wholesale';
    wc.newTaxIncluded = '0';
    await wc.createList(new Event('submit'));
    const alta = comandos.find((c) => c.name === 'pricing.price_lists.create');
    expect(alta!.payload.tax_included).toBe(false);
  });

  it('the column is in the table, and it is a select filter (closed domain)', async () => {
    const el = await montar();
    const cols = (el as unknown as {
      listColumns: { key: string; filterType?: string; options?: { value: string }[] }[];
    }).listColumns;
    const basis = cols.find((c) => c.key === 'tax_included');
    expect(basis, 'the tax basis of a list is invisible in its own table').toBeTruthy();
    expect(basis?.filterType).toBe('select');
    expect(basis?.options?.map((o) => o.value)).toEqual(['1', '0']);
  });
});

describe('el alta sigue funcionando desde el panel', () => {
  it('crear manda pricing.price_lists.create y CIERRA el panel de la tabla', async () => {
    const el = await montar();
    const listas = tablas(el)[0];
    let cerrado = 0;
    listas.close = () => {
      cerrado += 1;
    };

    const wc = el as unknown as { newCode: string; newName: string; newCurrency: string; createList: (ev: Event) => Promise<void> };
    wc.newCode = 'VIP';
    wc.newName = 'Tarifa VIP';
    wc.newCurrency = 'eur';
    await wc.createList(new Event('submit'));

    const alta = comandos.find((c) => c.name === 'pricing.price_lists.create');
    expect(alta, 'no se mandó el alta de la lista').toBeTruthy();
    expect(alta!.payload.code).toBe('VIP');
    expect(alta!.payload.currency).toBe('EUR');
    expect(cerrado, 'el panel de alta se queda abierto tras crear').toBe(1);
  });
});

describe('la columna VALOR enseña la magnitud de cada regla CON su unidad (pricing#28)', () => {
  // La tabla decía «Importe fijo · VALOR 0» para una regla que restaba 5,00 € de cada ticket: la
  // columna leía `value` (la TASA en %) y nunca `amount_cents` (el DINERO). El helper está
  // probado aparte en `ui/lib/rule-value.test.ts`; esto comprueba que la COLUMNA lo usa — un
  // helper correcto que nadie llama no arregla ninguna pantalla.
  function columnaValor(el: HTMLElement & { shadowRoot: ShadowRoot }) {
    const cols = (el as unknown as {
      ruleColumns: { key: string; sortable?: boolean; filterable?: boolean;
                     format?: (r: Record<string, unknown>) => string }[];
    }).ruleColumns;
    return cols.find((c) => c.key === 'value')!;
  }

  it('una regla `fixed` de 500 céntimos sale como 5,00 € en la tabla, no como 0', async () => {
    const el = await montar();
    expect(columnaValor(el).format!({ rule_type: 'fixed', value: '0', amount_cents: '500' }))
      .toBe('5.00 €');
  });

  it('una regla `percent` de 10 sale como 10 %', async () => {
    const el = await montar();
    expect(columnaValor(el).format!({ rule_type: 'percent', value: '10', amount_cents: '0' }))
      .toBe('10 %');
  });

  it('la columna tiene `format` — sin él vuelve a pintar el campo crudo', async () => {
    const el = await montar();
    expect(columnaValor(el).format, 'la columna VALOR se quedó sin format').toBeTypeOf('function');
  });

  it('NO es ordenable: ordenar unidades mezcladas no significa nada', async () => {
    const el = await montar();
    expect(columnaValor(el).sortable ?? false).toBe(false);
  });

  it('NO es filtrable: el `range` corría sobre `value`, en POR CIENTO', async () => {
    const el = await montar();
    expect(columnaValor(el).filterable ?? false).toBe(false);
  });

  it('pero TIPO y PRIORIDAD siguen siendo ordenables y filtrables (son los ejes reales)', async () => {
    const el = await montar();
    const cols = (el as unknown as {
      ruleColumns: { key: string; sortable?: boolean; filterable?: boolean }[];
    }).ruleColumns;
    for (const key of ['rule_type', 'priority']) {
      const c = cols.find((x) => x.key === key)!;
      expect(c.sortable, `${key} dejó de ser ordenable`).toBe(true);
      expect(c.filterable, `${key} dejó de ser filtrable`).toBe(true);
    }
  });
});

// pricing#43: a Lit `class=${…}` binding on the «Code» ion-input rewrites the WHOLE class attribute
// each time the error mark comes or goes, and wipes the classes Ionic stamped on the host. Stencil
// only re-adds what its own render changes, so the focus state, «has value», the outline fill and the
// floating-label position are lost until the view reloads. jsdom runs no Ionic, so the test stamps
// those classes itself, as Ionic does on hydrate.
describe('pricing#43: the «Code» field keeps its Ionic look when marked with an error', () => {
  const IONIC = ['ios', 'md', 'input-fill-outline', 'input-label-placement-floating', 'has-focus', 'has-value', 'hydrated'];

  it('marking, clearing and re-marking the duplicate-code error never drops the Ionic host classes', async () => {
    const sdk = (globalThis as Record<string, unknown>).erplora as Record<string, unknown>;
    sdk.command = async () => {
      throw Object.assign(new Error('duplicate'), { code: 'pricing.duplicate_code' });
    };
    const el = await montar();
    const code = () => el.shadowRoot.querySelector<HTMLElement>('[data-testid="pricing-code"]')!;
    code().classList.add(...IONIC);
    const wc = el as unknown as { newCode: string; newName: string; createList: (ev: Event) => Promise<void>; updateComplete: Promise<unknown> };
    const lost = () => IONIC.filter((c) => !code().classList.contains(c));
    const marked = () => ['ion-invalid', 'ion-touched'].every((c) => code().classList.contains(c));
    const submit = async () => {
      wc.newCode = 'BASE';
      wc.newName = 'Tarifa base';
      await wc.createList(new Event('submit'));
      await wc.updateComplete;
    };

    await submit();
    expect(marked(), 'the duplicate code is marked on the field').toBe(true);
    expect(lost(), 'Ionic classes lost when the error is marked').toEqual([]);

    (code() as unknown as { value: string }).value = 'BASE2';
    code().dispatchEvent(new CustomEvent('ionInput'));
    await wc.updateComplete;
    expect(marked(), 'typing clears the error mark').toBe(false);
    expect(code().classList.contains('ion-invalid'), 'ion-invalid stays after typing').toBe(false);
    expect(lost(), 'Ionic classes lost when the error is cleared').toEqual([]);

    await submit();
    expect(marked(), 'the error is marked again').toBe(true);
    expect(lost(), 'Ionic classes lost when the error is marked again').toEqual([]);
  });
});

// pricing#46: the create panel is short on a desktop, so after pressing «Add» at its bottom the
// «Code» field —and the duplicate-code message hanging from it— sits scrolled out of view: the
// rejection looked silent. The field that failed takes the focus (Ionic's `setFocus()`), which
// scrolls the panel up to it, as every mainstream form does with its first invalid field.
describe('pricing#46: a repeated code brings the «Code» field and its message into view', () => {
  const mount = async () => {
    const el = await montar();
    const code = el.shadowRoot.querySelector<HTMLElement>('[data-testid="pricing-code"]')!;
    const focus: string[] = [];
    // jsdom runs no Ionic: record what `setFocus` is asked, and whether the message was already
    // painted on the field at that moment (focusing before the render would show an empty field).
    (code as unknown as { setFocus: () => Promise<void> }).setFocus = async () => {
      focus.push(code.getAttribute('error-text') ?? '');
    };
    const wc = el as unknown as { newCode: string; newName: string; createList: (ev: Event) => Promise<void>; updateComplete: Promise<unknown> };
    const submit = async () => {
      wc.newCode = 'OFERTAS';
      wc.newName = 'Ofertas';
      await wc.createList(new Event('submit'));
      await wc.updateComplete;
      await new Promise((r) => setTimeout(r, 0));
    };
    return { focus, submit };
  };
  const sdk = () => (globalThis as Record<string, unknown>).erplora as Record<string, unknown>;

  it('the duplicate-code rejection focuses the «Code» field, with its message already painted', async () => {
    sdk().command = async () => {
      throw Object.assign(new Error('duplicate'), { code: 'pricing.duplicate_code' });
    };
    const { focus, submit } = await mount();
    await submit();
    expect(focus.length, 'setFocus calls on «Code»').toBe(1);
    expect(focus[0], 'the error-text was already painted when the focus landed').not.toBe('');
  });

  it('a failure that is not about a field does not steal the focus', async () => {
    sdk().command = async () => {
      throw Object.assign(new Error('boom'), { code: 'internal' });
    };
    const { focus, submit } = await mount();
    await submit();
    expect(focus).toEqual([]);
  });

  it('a successful create does not focus anything', async () => {
    const { focus, submit } = await mount();
    await submit();
    expect(focus).toEqual([]);
  });
});
