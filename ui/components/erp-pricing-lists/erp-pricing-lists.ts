import { LitElement, html, css, nothing } from 'lit';
import { state } from 'lit/decorators.js';
import { define } from '@erplora/outfitkit/define';
import '@erplora/outfitkit/ok-data-table';
import type { DataTableColumn } from '@erplora/outfitkit';
import { createListController } from '@erplora/module-sdk';
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
}

interface DiscountRule {
  id: string;
  code: string;
  name: string;
  rule_type: string;
  value: string;
  priority: number;
}

// Dominios cerrados de la migración (001_init.sql): el segmento de una lista y el tipo de una regla
// de descuento. Se ELIGEN (select), y el servidor los declara `op: eq` → el filtro es real.
const SEGMENTS = ['customer', 'business', 'wholesale', 'retail'];
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

  @state() newCode = '';

  @state() newName = '';

  @state() newCurrency = 'EUR';

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
    { key: 'value', header: t('ui.colValue'), align: 'right', sortable: true, filterable: true, filterType: 'range' },
    { key: 'priority', header: t('ui.colPriority'), align: 'right', sortable: true, filterable: true, filterType: 'text' },
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
    try {
      await erplora().command('pricing.price_lists.create', {
        code: this.newCode.trim(),
        name: this.newName.trim(),
        currency: (this.newCurrency || 'EUR').trim().toUpperCase(),
        segment: null,
        is_default: false,
        valid_from: null,
        valid_until: null,
      });
      this.newCode = '';
      this.newName = '';
      this.newCurrency = 'EUR';
      this.dataTable()?.close(); // el panel de alta se cierra solo tras crear
      await this.listsCtrl.load(); // (además del evento; garantiza refresco inmediato)
    } catch (e) {
      this.formError = e instanceof Error ? e.message : erplora().t(CATALOG, 'ui.createListError');
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
        ${this.formError ? html`<p class="err">${this.formError}</p>` : nothing}
        ${this.listsCtrl?.error ? html`<p class="err">${this.listsCtrl.error}</p>` : nothing}
        <ok-data-table .serverSide=${true} .fill=${true} .addable=${true} .columns=${this.listColumns} .rows=${this.listsCtrl?.rows ?? []} .total=${this.listsCtrl?.total ?? 0} .page=${this.listsCtrl?.state.page ?? 0} .pageSize=${this.listsCtrl?.state.pageSize ?? 50} .sort=${this.listsCtrl?.state.sort} .sortDir=${this.listsCtrl?.state.dir ?? 'asc'} .searchable=${true} .searchPlaceholder=${t('ui.searchPlaceholder')} .emptyMessage=${this.listsCtrl?.loading ? t('ui.loading') : t('ui.emptyLists')} @pageChange=${(e: CustomEvent<number>) => this.listsCtrl.setPage(e.detail)} @pageSizeChange=${(e: CustomEvent<number>) => this.listsCtrl.setPageSize(e.detail)} @sortChange=${(e: CustomEvent<{ sort: string; dir: 'asc' | 'desc' }>) => this.listsCtrl.setSort(e.detail.sort, e.detail.dir)} @searchChange=${(e: CustomEvent<string>) => this.listsCtrl.setSearch(e.detail)} @filterChange=${(e: CustomEvent<{ col: string; value: unknown }>) => this.listsCtrl.setFilter(e.detail.col, e.detail.value)}>
          <!-- Alta: se proyecta SIEMPRE (aunque el panel esté cerrado); si solo se pintara al abrir,
               el «+» de la barra desplegaría un panel vacío. -->
          <form slot="create" class="form" @submit=${(e: Event) => this.createList(e)}>
            <ion-input fill="outline" label-placement="floating" label=${t('ui.colCode')} .value=${this.newCode} @ionInput=${(e: any) => (this.newCode = e.target.value)}></ion-input>
            <ion-input fill="outline" label-placement="floating" label=${t('ui.colName')} .value=${this.newName} @ionInput=${(e: any) => (this.newName = e.target.value)}></ion-input>
            <ion-input fill="outline" label-placement="floating" label=${t('ui.colCurrency')} .value=${this.newCurrency} @ionInput=${(e: any) => (this.newCurrency = e.target.value)}></ion-input>
            <ion-button type="submit" ?disabled=${this.saving || !this.newCode || !this.newName}>${this.saving ? t('ui.btnSaving') : t('ui.btnAdd')}</ion-button>
          </form>
        </ok-data-table>
        <h3>${t('ui.rulesTitle')}</h3>
        ${this.rulesCtrl?.error ? html`<p class="err">${this.rulesCtrl.error}</p>` : nothing}
        <ok-data-table .serverSide=${true} .fill=${true} .addable=${false} .columns=${this.ruleColumns} .rows=${this.rulesCtrl?.rows ?? []} .total=${this.rulesCtrl?.total ?? 0} .page=${this.rulesCtrl?.state.page ?? 0} .pageSize=${this.rulesCtrl?.state.pageSize ?? 50} .sort=${this.rulesCtrl?.state.sort} .sortDir=${this.rulesCtrl?.state.dir ?? 'asc'} .searchable=${true} .searchPlaceholder=${t('ui.searchPlaceholder')} .emptyMessage=${this.rulesCtrl?.loading ? t('ui.loading') : t('ui.emptyRules')} @pageChange=${(e: CustomEvent<number>) => this.rulesCtrl.setPage(e.detail)} @pageSizeChange=${(e: CustomEvent<number>) => this.rulesCtrl.setPageSize(e.detail)} @sortChange=${(e: CustomEvent<{ sort: string; dir: 'asc' | 'desc' }>) => this.rulesCtrl.setSort(e.detail.sort, e.detail.dir)} @searchChange=${(e: CustomEvent<string>) => this.rulesCtrl.setSearch(e.detail)} @filterChange=${(e: CustomEvent<{ col: string; value: unknown }>) => this.rulesCtrl.setFilter(e.detail.col, e.detail.value)}></ok-data-table>
      </div>`;
  }
}

define('erp-pricing-lists', ErpPricingLists);
