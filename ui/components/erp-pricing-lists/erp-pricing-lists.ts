import { LitElement, html, css, nothing } from 'lit';
import { state } from 'lit/decorators.js';
import { define } from '@erplora/outfitkit/define';
import '@erplora/outfitkit/ok-data-table';
import type { DataTableColumn } from '@erplora/outfitkit';
import { createListController } from '@erplora/module-sdk';
import type { ListController, ListClient, ListParams, ListPage } from '@erplora/module-sdk';

interface ErploraClientLike extends ListClient {
  query<T = unknown>(name: string, params?: Record<string, unknown>): Promise<T>;
  queryPage<R = unknown>(name: string, params: ListParams): Promise<ListPage<R>>;
  command<T = unknown>(name: string, payload?: Record<string, unknown>): Promise<T>;
  on(event: string, cb: (payload: unknown) => void): () => void;
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

function erplora(): ErploraClientLike {
  const c = (globalThis as { erplora?: ErploraClientLike }).erplora;
  if (!c) throw new Error('erplora SDK no inicializado por el shell');
  return c;
}

export class ErpPricingLists extends LitElement {
  static styles = css`
    :host { display:block; font-family: system-ui, sans-serif; color: var(--ink, #1c1b18); }
    header { display:flex; gap:.5rem; align-items:center; margin-bottom:.75rem; }
    h2 { margin:0; font-size:1.15rem; flex:1; }
    h3 { margin:1.5rem 0 .5rem; font-size:1rem; color:var(--muted,#5c594f); }
    .form { display:flex; gap:.5rem; flex-wrap:wrap; align-items:end; margin:.5rem 0 1rem; }
    .form ion-input { --background:var(--surface-2,#f7f4ec); border:1px solid var(--line,#e7e2d6); border-radius:8px; min-width:8rem; }
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

  private listColumns: DataTableColumn[] = [
    {
      key: 'code',
      header: 'Código',
      sortable: true,
      filterable: true,
      filterType: 'text',
      format: (r) => `${r.code}${r.is_default ? '  (default)' : ''}`,
    },
    { key: 'name', header: 'Nombre', sortable: true, filterable: true, filterType: 'text' },
    { key: 'currency', header: 'Divisa', sortable: true, filterable: true, filterType: 'text' },
    {
      key: 'segment',
      header: 'Segmento',
      sortable: true,
      filterable: true,
      filterType: 'text',
      format: (r) => (r.segment as string) ?? '—',
    },
  ];

  private ruleColumns: DataTableColumn[] = [
    { key: 'code', header: 'Código', sortable: true, filterable: true, filterType: 'text' },
    { key: 'name', header: 'Nombre', sortable: true, filterable: true, filterType: 'text' },
    { key: 'rule_type', header: 'Tipo', sortable: true, filterable: true, filterType: 'text' },
    { key: 'value', header: 'Valor', align: 'right', sortable: true, filterable: true, filterType: 'range' },
    { key: 'priority', header: 'Prioridad', align: 'right', sortable: true, filterable: true, filterType: 'text' },
  ];

  // TODO-LIT: componentWillLoad → connectedCallback. Recuerda: connectedCallback se dispara
  // en CADA reconexión al DOM (no solo en el primer montaje). Si la init debe correr una
  // sola vez tras el primer render, considera firstUpdated() en su lugar.
  async connectedCallback() {
    super.connectedCallback();
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
    super.disconnectedCallback();
    this.unsub?.();
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
      await this.listsCtrl.load(); // (además del evento; garantiza refresco inmediato)
    } catch (e) {
      this.formError = e instanceof Error ? e.message : 'No se pudo crear la lista';
    } finally {
      this.saving = false;
    }
  }

  render() {
    return html`<div>
        <header>
          <h2>Listas de precios</h2>
        </header>
        <form class="form" @submit=${(e) => this.createList(e)}>
          <ion-input placeholder="Código" .value=${this.newCode} @ionInput=${(e: any) => (this.newCode = e.target.value)}></ion-input>
          <ion-input placeholder="Nombre" .value=${this.newName} @ionInput=${(e: any) => (this.newName = e.target.value)}></ion-input>
          <ion-input placeholder="Divisa" .value=${this.newCurrency} @ionInput=${(e: any) => (this.newCurrency = e.target.value)}></ion-input>
          <ion-button type="submit" size="small" ?disabled=${this.saving || !this.newCode || !this.newName}>${this.saving ? 'Guardando…' : 'Añadir'}</ion-button>
        </form>
        ${this.formError ? html`<p class="err">${this.formError}</p>` : nothing}
        ${this.listsCtrl?.error ? html`<p class="err">${this.listsCtrl.error}</p>` : nothing}
        <ok-data-table .serverSide=${true} .columns=${this.listColumns} .rows=${this.listsCtrl?.rows ?? []} .total=${this.listsCtrl?.total ?? 0} .page=${this.listsCtrl?.state.page ?? 0} .pageSize=${this.listsCtrl?.state.pageSize ?? 50} .sort=${this.listsCtrl?.state.sort} .sortDir=${this.listsCtrl?.state.dir ?? 'asc'} .searchable=${true} .searchPlaceholder=${"Buscar código o nombre…"} .emptyMessage=${this.listsCtrl?.loading ? 'Cargando…' : 'Sin listas de precios.'} @pageChange=${(e: CustomEvent<number>) => this.listsCtrl.setPage(e.detail)} @sortChange=${(e: CustomEvent<{ sort: string; dir: 'asc' | 'desc' }>) => this.listsCtrl.setSort(e.detail.sort, e.detail.dir)} @searchChange=${(e: CustomEvent<string>) => this.listsCtrl.setSearch(e.detail)} @filterChange=${(e: CustomEvent<{ col: string; value: unknown }>) => this.listsCtrl.setFilter(e.detail.col, e.detail.value)}></ok-data-table>
        <h3>Reglas de descuento</h3>
        ${this.rulesCtrl?.error ? html`<p class="err">${this.rulesCtrl.error}</p>` : nothing}
        <ok-data-table .serverSide=${true} .columns=${this.ruleColumns} .rows=${this.rulesCtrl?.rows ?? []} .total=${this.rulesCtrl?.total ?? 0} .page=${this.rulesCtrl?.state.page ?? 0} .pageSize=${this.rulesCtrl?.state.pageSize ?? 50} .sort=${this.rulesCtrl?.state.sort} .sortDir=${this.rulesCtrl?.state.dir ?? 'asc'} .searchable=${true} .searchPlaceholder=${"Buscar código o nombre…"} .emptyMessage=${this.rulesCtrl?.loading ? 'Cargando…' : 'Sin reglas de descuento.'} @pageChange=${(e: CustomEvent<number>) => this.rulesCtrl.setPage(e.detail)} @sortChange=${(e: CustomEvent<{ sort: string; dir: 'asc' | 'desc' }>) => this.rulesCtrl.setSort(e.detail.sort, e.detail.dir)} @searchChange=${(e: CustomEvent<string>) => this.rulesCtrl.setSearch(e.detail)} @filterChange=${(e: CustomEvent<{ col: string; value: unknown }>) => this.rulesCtrl.setFilter(e.detail.col, e.detail.value)}></ok-data-table>
      </div>`;
  }
}

define('erp-pricing-lists', ErpPricingLists);
