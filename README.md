# Módulo `pricing` — listas de precios y reglas de descuento

Centraliza **cuánto cuesta algo y por qué**: **listas de precios** por divisa/segmento/vigencia con
precio por producto y **bracket de cantidad**, y **reglas** de descuento/recargo evaluadas por
prioridad. Más dos calculadoras: precio final de un producto y descuento aplicable a un importe,
**repartido línea a línea y agregado por tipo impositivo**.

> 🧾 **ADR-0210 — un precio NO es un entero, y un descuento NO es un número suelto.** Cada lista
> declara su **base fiscal** (`tax_included` tri-estado: bruto · base imponible · **heredar del
> hub**) y toda respuesta la lleva **resuelta y con su procedencia**. Mezclar bases es un **error**
> (`mixed_tax_basis`), no una comparación silenciosa. Y repartir un descuento se hace por **RESTO
> MAYOR**, que conserva la suma al céntimo — HALF_UP (el redondeo del dinero, ADR-0123) **no** la
> conserva al partir.

> **Module id:** `pricing`. **Depende de:** nada — y nada depende de él.
> Módulo híbrido: SQL + handler WASM (`create_price_list` escribe; `get_price` y
> `calculate_discount` leen de `context.reads`, ADR-0069).

## Documentación de usuario — [`docs/`](docs/)

Viaja **dentro** del módulo y se versiona con él: el asistente del hub (ADR-0282) la indexa por
versión instalada y cita la de TU versión, no la de la última publicada. En inglés (idioma fuente).

| Fichero | Para qué |
| ------- | -------- |
| [`docs/overview.md`](docs/overview.md) | Qué hace y qué NO hace; las dos calculadoras |
| [`docs/screens.md`](docs/screens.md) | Price Lists, items por bracket, reglas y cómo se pide precio/descuento |
| [`docs/concepts.md`](docs/concepts.md) | La base fiscal viaja con el precio, `mixed_tax_basis` es un ERROR, **repartir ≠ redondear** (resto mayor), el orden de cálculo es FIJO, la base se CONGELA en la línea |
| [`docs/limits.md`](docs/limits.md) | Los 8 códigos de error, la limitación del canal de resultado y permisos por acción |

## El reparto (ADR-0210)

Resto mayor (Hamilton), en enteros: cada línea se lleva el **suelo** de su parte y los céntimos
sobrantes van de uno en uno a las de mayor parte fraccionaria. Invariantes fijados por tests:
`Σ allocation[].discount == total_discount` exacto · `Σ by_tax_rate[].amount_after == final_amount` ·
ninguna línea baja de 0 · **el signo viaja** (un recargo sigue siendo recargo) · pedido todo-a-cero
reparte 0 · **determinista**.

## Qué expone hoy

| Tipo | Nombre | Permiso |
| ---- | ------ | ------- |
| query | `pricing.price_lists.list` / `.get` / `.items` / `.items_by_product` · `pricing.rules.list` | `view_pricing` |
| command | `pricing.price_lists.create` (WASM, invariante «una sola default») | `manage_pricing` |
| command | `pricing.price_lists.add_item` (→ `pricing.price_list_unavailable`) · `pricing.rules.create` / `.deactivate` | `manage_pricing` |
| command | `pricing.price_lists.get_price` (WASM, solo lectura) · `pricing.rules.calculate_discount` (WASM, solo lectura) | `apply_pricing` |
| emite | `pricing.price_list.created`, `pricing.price_item.added`, `pricing.rule.created`, `pricing.rule.deactivated` | — |
| escucha | — | — |

Navegación: `erp-pricing-lists` («Price Lists»).

## Layout

```text
module.json                   # manifest (contrato técnico)
migrations/postgres/          # esquema §2.5 + 003_price_list_tax_basis.sql (tri-estado)
queries/*.sql                 # lecturas declarativas (:hub_id inyectado)
commands/*.sql                # escrituras declarativas (las `_` son intenciones del WASM)
schemas/*.json                # JSON Schemas de input (draft 2020-12)
handler/                      # WASM Tier 2 (rust_decimal + guest_sdk::money) → dist/handler.wasm
ui/                           # Web Components (Lit/Ionic/OutfitKit)
docs/                         # documentación de usuario + corpus del asistente
```

## Estado y trabajo abierto

El estado vive en las **Issues de este repo**, no aquí. Limitación documentada en `docs/limits.md`:
el host **ignora el campo `result`** del handler, así que el veredicto de `get_price` y
`calculate_discount` no llega al caller — el contrato está escrito, compilado y testeado, y se
recogerá **sin tocar el manifest** cuando el dispatcher devuelva resultados de handlers read-only.

Doc de arquitectura: `architecture/modules/pricing.md` +
[`architecture/contracts/money-contract.md`](../../../architecture/contracts/money-contract.md) §7.
