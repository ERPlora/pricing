import { LitElement, html, css, nothing } from 'lit';
import { state } from 'lit/decorators.js';
import { classMap } from 'lit/directives/class-map.js';
import { define } from '@erplora/outfitkit/define';
import '@erplora/outfitkit/ok-data-table';
import type { DataTableColumn } from '@erplora/outfitkit';
import { createListController } from '@erplora/module-sdk';
// Traducción del rechazo del servidor a algo que el usuario pueda leer y corregir (pricing#29).
import { commandError } from '../../lib/command-error';
// La columna VALOR enseña la magnitud de cada regla CON su unidad (pricing#28).
import { ruleValueLabel } from '../../lib/rule-value';
import type { ListController, ListClient, ListParams, ListPage } from '@erplora/module-sdk';
// Catálogo i18n del módulo (ADR-0055): esbuild inlinea estos JSON en el `dist` del WC. Los textos
// internos se resuelven con `erplora.t(CATALOG, 'ui.clave')` (idioma activo, fallback locale→en→clave).
import esLocale from '../../../locales/es.json';
import enLocale from '../../../locales/en.json';
const CATALOG: Record<string, unknown> = { es: esLocale, en: enLocale };

interface ErploraClientLike extends ListClient {
  query<T = unknown>(name: string, params?: Record<string, unknown>): Promise<T>;
  queryPage<R = unknown>(name: string, params: ListParams): Promise<ListPage<R>>;
  command<T = unknown>(name: string, payload?: Record<string, unknown>): Promise<T>;
  on(event: string, cb: (payload: unknown) => void): () => void;
  /** Moneda del hub + formateo de dinero (ADR-0059). `formatMoney` recibe CÉNTIMOS. */
  formatMoney(cents: number, opts?: { currency?: string; locale?: string }): string;
  /** i18n del módulo (ADR-0055): idioma activo + traducción del catálogo `ui`. */
  locale: string;
  t(catalog: Record<string, unknown>, key: string, params?: Record<string, unknown>): string;
}

interface PriceList {
  id: string;
  code: string;
  name: string;
  currency: string;
  is_default: number;
  segment: string | null;
  /** ADR-0210 tri-state: 1 gross · 0 taxable base · null = inherit the hub. */
  tax_included: number | null;
}

interface DiscountRule {
  id: string;
  code: string;
  name: string;
  rule_type: string;
  /** TASA en % (reglas `percent`). Vale 0 en una regla `fixed` — ADR-0007/0123, migración 002. */
  value: string;
  /** DINERO en céntimos (reglas `fixed`). Vale 0 en una regla `percent`. Las dos unidades van
   *  SEPARADAS a propósito: cuando compartían columna, un descuento fijo de 5 € restaba 5
   *  CÉNTIMOS. `rules_list.sql` ya lo proyectaba; era la columna la que no lo leía (pricing#28). */
  amount_cents: string;
  priority: number;
}

// Dominios cerrados de la migración (001_init.sql): el segmento de una lista y el tipo de una regla
// de descuento. Se ELIGEN (select), y el servidor los declara `op: eq` → el filtro es real.
const SEGMENTS = ['customer', 'business', 'wholesale', 'retail'];
// ADR-0210: the two states a list can PIN. A third one exists —inherit the hub— but it is the
// ABSENCE of a value (NULL), so it is not a filter option: you cannot filter for "no answer".
const TAX_BASIS = ['1', '0'];
const RULE_TYPES = ['percent', 'fixed', 'buy_x_get_y', 'tiered'];

function erplora(): ErploraClientLike {
  const c = (globalThis as { erplora?: ErploraClientLike }).erplora;
  if (!c) throw new Error('erplora SDK no inicializado por el shell');
  return c;
}

export class ErpPricingLists extends LitElement {
  static styles = css`
    :host { display:flex; flex-direction:column; height:100%; min-height:0; font-family: system-ui, sans-serif; color: var(--ion-text-color, #1c1b18); }
    /* Dos tablas apiladas que se reparten el alto: cada una con su scroll interno y su pie fijo. */
    .page { display:flex; flex-direction:column; gap:.5rem; min-height:0; flex:1 1 auto; }
    .page > ok-data-table { flex:1 1 0; min-height:12rem; }
    h3 { margin:.5rem 0 0; font-size:1rem; color:var(--ion-color-medium,#5c594f); }
    /* El alta vive en el panel lateral de la tabla (estrecho): los campos van APILADOS. */
    .form { display:flex; flex-direction:column; gap:.7rem; }
    .form ion-button { align-self:flex-end; }
    .err { color:#d9480f; font-weight:600; }
  `;

  @state() formError = '';
  /** Campo del formulario al que cuelga `formError` (`'code'`), o '' si es del formulario entero
   *  (pricing#29). Un «ya existe ese código» tiene que marcar el campo Código, no una banda roja
   *  suelta arriba del listado que no dice cuál de los cuatro campos hay que tocar. */
  @state() formErrorField = '';

  @state() newCode = '';

  @state() newName = '';

  @state() newCurrency = 'EUR';

  // '' = inherit the hub (stored as NULL). A new list follows the hub unless the user pins it:
  // pinning by default would freeze today's setting into every list ever created.
  @state() newTaxIncluded: '' | '1' | '0' = '';

  @state() saving = false;

  @state() tick = 0;

  private listsCtrl!: ListController<PriceList>;

  private rulesCtrl!: ListController<DiscountRule>;

  private unsub?: () => void;

  // Getters (no campos): se re-evalúan en cada render, así los headers cambian con el idioma activo
  // (ADR-0055). `connectedCallback` re-renderiza al recibir `erplora:locale-changed`.
  private get listColumns(): DataTableColumn[] {
    const t = (k: string): string => erplora().t(CATALOG, k);
    return [
    {
      key: 'code',
      header: t('ui.colCode'),
      sortable: true,
      filterable: true,
      filterType: 'text',
      format: (r) => `${r.code}${r.is_default ? `  ${t('ui.defaultBadge')}` : ''}`,
    },
    { key: 'name', header: t('ui.colName'), sortable: true, filterable: true, filterType: 'text' },
    { key: 'currency', header: t('ui.colCurrency'), sortable: true, filterable: true, filterType: 'text' },
    {
      key: 'segment',
      header: t('ui.colSegment'),
      sortable: true,
      filterable: true,
      filterType: 'select',
      options: SEGMENTS.map((v) => ({ value: v, label: t(`ui.segment.${v}`) })),
      format: (r) => (r.segment ? t(`ui.segment.${String(r.segment)}`) : '—'),
    },
    {
      key: 'tax_included',
      header: t('ui.colTaxBasis'),
      sortable: true,
      filterable: true,
      filterType: 'select',
      options: TAX_BASIS.map((v) => ({ value: v, label: t(`ui.taxBasis.${v === '1' ? 'included' : 'excluded'}`) })),
      // NULL is not "unknown": it is "inherit the hub", and it is said out loud so nobody has to
      // guess what a blank cell means.
      format: (r) =>
        r.tax_included === null || r.tax_included === undefined
          ? t('ui.taxBasis.inherit')
          : t(`ui.taxBasis.${Number(r.tax_included) === 1 ? 'included' : 'excluded'}`),
    },
    ];
  }

  private get ruleColumns(): DataTableColumn[] {
    const t = (k: string): string => erplora().t(CATALOG, k);
    return [
    { key: 'code', header: t('ui.colCode'), sortable: true, filterable: true, filterType: 'text' },
    { key: 'name', header: t('ui.colName'), sortable: true, filterable: true, filterType: 'text' },
    {
      key: 'rule_type',
      header: t('ui.colType'),
      sortable: true,
      filterable: true,
      filterType: 'select',
      options: RULE_TYPES.map((v) => ({ value: v, label: t(`ui.ruleType.${v}`) })),
      format: (r) => t(`ui.ruleType.${String(r.rule_type)}`),
    },
    {
      // NI `sortable` NI `filterable`, a propósito (pricing#28). Ordenar una columna que mezcla
      // unidades no significa nada —¿es 10 % mayor que 5 €?— y el filtro `range` corría sobre
      // `value`, en POR CIENTO, así que un «≥ 5» no encontraba la regla de 5 €. El eje por el que
      // de verdad se agrupa es `rule_type`, que sigue siendo ordenable y filtrable, y `priority`,
      // que es lo que decide qué regla gana.
      key: 'value',
      header: t('ui.colValue'),
      align: 'right',
      format: (r) => ruleValueLabel(r, (c) => erplora().formatMoney(c), t),
    },
    { key: 'priority', header: t('ui.colPriority'), align: 'right', sortable: true, filterable: true, filterType: 'range' },
    ];
  }

  // Re-render al cambiar el idioma del shell (ADR-0055): los getters `listColumns`/`ruleColumns` y
  // el texto del template se re-evalúan con el nuevo `erplora.locale`.
  private readonly onLocaleChange = (): void => this.requestUpdate();

  // TODO-LIT: componentWillLoad → connectedCallback. Recuerda: connectedCallback se dispara
  // en CADA reconexión al DOM (no solo en el primer montaje). Si la init debe correr una
  // sola vez tras el primer render, considera firstUpdated() en su lugar.
  async connectedCallback() {
    super.connectedCallback();
    window.addEventListener('erplora:locale-changed', this.onLocaleChange);
    this.listsCtrl = createListController<PriceList>(erplora(), 'pricing.price_lists.list', () => this.requestUpdate(), {
      pageSize: 50,
      sort: 'name',
      dir: 'asc',
    });
    this.rulesCtrl = createListController<DiscountRule>(erplora(), 'pricing.rules.list', () => this.requestUpdate(), {
      pageSize: 50,
      sort: 'name',
      dir: 'asc',
    });
    await Promise.all([this.listsCtrl.load(), this.rulesCtrl.load()]);
    // Reactividad: recargamos cuando el runtime emite cambios de pricing.
    try {
      const off1 = erplora().on('pricing.price_list.created', () => this.listsCtrl.load());
      const off2 = erplora().on('pricing.rule.created', () => this.rulesCtrl.load());
      const off3 = erplora().on('pricing.rule.deactivated', () => this.rulesCtrl.load());
      this.unsub = () => {
        off1();
        off2();
        off3();
      };
    } catch {
      /* sin SDK (preview) → sin reactividad en vivo */
    }
  }

  disconnectedCallback() {
    window.removeEventListener('erplora:locale-changed', this.onLocaleChange);
    super.disconnectedCallback();
    this.unsub?.();
  }

  // Referencia a la tabla de LISTAS (la primera): es la que lleva el «+» y proyecta el alta.
  private dataTable(): { open(p?: 'filters' | 'create'): void; close(): void } | null {
    return this.renderRoot.querySelector('ok-data-table') as
      | { open(p?: 'filters' | 'create'): void; close(): void }
      | null;
  }

  private async createList(ev: Event) {
    ev.preventDefault();
    if (!this.newCode.trim() || !this.newName.trim()) return;
    this.saving = true;
    this.formError = '';
    this.formErrorField = '';
    try {
      await erplora().command('pricing.price_lists.create', {
        code: this.newCode.trim(),
        name: this.newName.trim(),
        currency: (this.newCurrency || 'EUR').trim().toUpperCase(),
        segment: null,
        is_default: false,
        // '' → null: "inherit the hub", NOT a resolved value (ADR-0210).
        tax_included: this.newTaxIncluded === '' ? null : this.newTaxIncluded === '1',
        valid_from: null,
        valid_until: null,
      });
      this.newCode = '';
      this.newName = '';
      this.newCurrency = 'EUR';
      this.newTaxIncluded = '';
      this.dataTable()?.close(); // el panel de alta se cierra solo tras crear
      await this.listsCtrl.load(); // (además del evento; garantiza refresco inmediato)
    } catch (e) {
      // NUNCA `e.message` en crudo: ese era el camino por el que salía a pantalla el
      // «db: sqlx: … duplicate key value violates unique constraint … at line 666».
      const shown = commandError(e, (k) => erplora().t(CATALOG, k));
      this.formError = shown.message;
      this.formErrorField = shown.field ?? '';
      // pricing#46: on a desktop the panel stays scrolled down at «Add», with the failing field
      // out of view. Focus it once its message is painted: Ionic scrolls the panel up to it.
      if (shown.field) {
        await this.updateComplete;
        const field = this.renderRoot.querySelector(`[data-testid="pricing-${shown.field}"]`) as
          | (HTMLElement & { setFocus?: () => Promise<void> })
          | null;
        await field?.setFocus?.();
      }
    } finally {
      this.saving = false;
    }
  }

  // El título de la vista lo pinta el topbar del shell: repetirlo aquí lo duplicaba en pantalla.
  // La tabla de reglas de descuento NO declara `addable`: esta vista no da de alta reglas, y un «+»
  // que abre un panel vacío es peor que ningún «+».
  render() {
    const t = (k: string): string => erplora().t(CATALOG, k);
    return html`<div class="page">
        ${this.formError && !this.formErrorField ? html`<p class="err" data-testid="pricing-form-error">${this.formError}</p>` : nothing}
        ${this.listsCtrl?.error ? html`<p class="err" data-testid="pricing-load-error">${this.listsCtrl.error}</p>` : nothing}
        <ok-data-table testid="pricing-table" .serverSide=${true} .fill=${true} .addable=${true} .views=${true} .cardTitle=${(row: Record<string, unknown>) => String(row.name ?? row.code ?? '—')} .columns=${this.listColumns} .rows=${this.listsCtrl?.rows ?? []} .total=${this.listsCtrl?.total ?? 0} .page=${this.listsCtrl?.state.page ?? 0} .pageSize=${this.listsCtrl?.state.pageSize ?? 50} .sort=${this.listsCtrl?.state.sort} .sortDir=${this.listsCtrl?.state.dir ?? 'asc'} .searchable=${true} .searchPlaceholder=${t('ui.searchPlaceholder')} .emptyMessage=${this.listsCtrl?.loading ? t('ui.loading') : t('ui.emptyLists')} @pageChange=${(e: CustomEvent<number>) => this.listsCtrl.setPage(e.detail)} @pageSizeChange=${(e: CustomEvent<number>) => this.listsCtrl.setPageSize(e.detail)} @sortChange=${(e: CustomEvent<{ sort: string; dir: 'asc' | 'desc' }>) => this.listsCtrl.setSort(e.detail.sort, e.detail.dir)} @searchChange=${(e: CustomEvent<string>) => this.listsCtrl.setSearch(e.detail)} @filterChange=${(e: CustomEvent<{ col: string; value: unknown }>) => this.listsCtrl.setFilter(e.detail.col, e.detail.value)}>
          <!-- Alta: se proyecta SIEMPRE (aunque el panel esté cerrado); si solo se pintara al abrir,
               el «+» de la barra desplegaría un panel vacío. -->
          <form slot="create" class="form" data-testid="pricing-form" @submit=${(e: Event) => this.createList(e)}>
            <ion-input mode="md" fill="outline" label-placement="floating" label=${t('ui.colCode')}
              data-testid="pricing-code"
              class=${classMap({ 'ion-invalid': this.formErrorField === 'code', 'ion-touched': this.formErrorField === 'code' })}
              error-text=${this.formErrorField === 'code' ? this.formError : ''}
              .value=${this.newCode} @ionInput=${(e: any) => { this.newCode = e.target.value; if (this.formErrorField === 'code') { this.formError = ''; this.formErrorField = ''; } }}></ion-input>
            <ion-input mode="md" fill="outline" label-placement="floating" label=${t('ui.colName')} data-testid="pricing-name" .value=${this.newName} @ionInput=${(e: any) => (this.newName = e.target.value)}></ion-input>
            <ion-input mode="md" fill="outline" label-placement="floating" label=${t('ui.colCurrency')} data-testid="pricing-currency" .value=${this.newCurrency} @ionInput=${(e: any) => (this.newCurrency = e.target.value)}></ion-input>
            <ion-select mode="md" fill="outline" label-placement="floating" label=${t('ui.colTaxBasis')} data-testid="pricing-tax-basis" .value=${this.newTaxIncluded} @ionChange=${(e: any) => (this.newTaxIncluded = e.target.value)}>
              <ion-select-option value="">${t('ui.taxBasis.inherit')}</ion-select-option>
              <ion-select-option value="1">${t('ui.taxBasis.included')}</ion-select-option>
              <ion-select-option value="0">${t('ui.taxBasis.excluded')}</ion-select-option>
            </ion-select>
            <ion-button type="submit" data-testid="pricing-submit" ?disabled=${this.saving || !this.newCode || !this.newName}>${this.saving ? t('ui.btnSaving') : t('ui.btnAdd')}</ion-button>
          </form>
        </ok-data-table>
        <h3>${t('ui.rulesTitle')}</h3>
        ${this.rulesCtrl?.error ? html`<p class="err" data-testid="pricing-rules-load-error">${this.rulesCtrl.error}</p>` : nothing}
        <ok-data-table testid="pricing-rules-table" .serverSide=${true} .fill=${true} .addable=${false} .views=${true} .cardTitle=${(row: Record<string, unknown>) => String(row.name ?? row.code ?? '—')} .columns=${this.ruleColumns} .rows=${this.rulesCtrl?.rows ?? []} .total=${this.rulesCtrl?.total ?? 0} .page=${this.rulesCtrl?.state.page ?? 0} .pageSize=${this.rulesCtrl?.state.pageSize ?? 50} .sort=${this.rulesCtrl?.state.sort} .sortDir=${this.rulesCtrl?.state.dir ?? 'asc'} .searchable=${true} .searchPlaceholder=${t('ui.searchPlaceholder')} .emptyMessage=${this.rulesCtrl?.loading ? t('ui.loading') : t('ui.emptyRules')} @pageChange=${(e: CustomEvent<number>) => this.rulesCtrl.setPage(e.detail)} @pageSizeChange=${(e: CustomEvent<number>) => this.rulesCtrl.setPageSize(e.detail)} @sortChange=${(e: CustomEvent<{ sort: string; dir: 'asc' | 'desc' }>) => this.rulesCtrl.setSort(e.detail.sort, e.detail.dir)} @searchChange=${(e: CustomEvent<string>) => this.rulesCtrl.setSearch(e.detail)} @filterChange=${(e: CustomEvent<{ col: string; value: unknown }>) => this.rulesCtrl.setFilter(e.detail.col, e.detail.value)}></ok-data-table>
      </div>`;
  }
}

define('erp-pricing-lists', ErpPricingLists);
