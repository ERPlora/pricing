# WORKFLOW — Precios

Prefijo: PRICING
Alcance MVP: fuera del MVP

> Oleada 5: se documenta **lo que hay** en `origin/main`, sin ampliar. Nada de lo que sigue es una
> propuesta. Este módulo está instalable, pero **ningún otro módulo lo consulta** (ver «Datos»).

## Para qué sirve y para quién
Guarda **tarifas** (listas de precios por artículo, con tramos de cantidad, divisa, segmento de
cliente y base fiscal) y **reglas de descuento o recargo** (porcentaje, importe fijo, por tramos,
«compra X, llévate Y"), y sabe calcular el precio de un artículo en las tarifas y el descuento de un
importe. Lo usa el **responsable** (el administrador y el responsable gestionan; el empleado solo
ve y pide cálculos). Instalarlo **no cambia lo que cobra Vender**: el precio de la línea de un
tique sigue saliendo de Inventario o de Servicios, y los descuentos de Vender siguen siendo los
manuales de Vender. Este módulo es un motor que alguien tiene que llamar, y hoy nadie lo llama.

## Referencia adoptada
Listas de precios y reglas de descuento como en Odoo (tarifas con tramos y reglas por prioridad) y
Business Central (precio de venta por tramo de cantidad, base fiscal explícita); la pantalla de
reglas compara contra Square, Shopify, Toast, Lightspeed y WooCommerce (comparativa ya recogida en
`ui/lib/rule-value.ts`). La base fiscal y el reparto del descuento a las líneas salen de ADR-0210 y
del contrato de dinero (`architecture/contracts/money-contract.md`). Sin investigación nueva.

## Antes de empezar
- No depende de ningún módulo (`depends_on` vacío) y ningún módulo depende de él.
- El administrador y el responsable tienen los tres permisos; el empleado, ver y aplicar.
- No hay configuración inicial ni tarifa sembrada: hay que crear la primera tarifa (PRICING-F02).
- Para tener algo que consultar hay que dar de alta precios de artículo (PRICING-F03, solo por el
  asistente o la API: la pantalla no lo ofrece).

## Pantallas
**Tarifas** (única entrada del menú, icono de etiqueta). Dos tablas apiladas:
- **Listas de precios**: Código (con «(default)» si es la tarifa por defecto), Nombre, Divisa,
  Segmento (Particular, Empresa, Mayorista, Minorista o «—»), Impuesto (Según el hub, Impuesto
  incluido, Impuesto no incluido). Búsqueda por código o nombre, orden y filtros por columna,
  50 por página. Botón «+» para el alta (PRICING-F02). Vacía: «Sin listas de precios.»; cargando:
  «Cargando…»; con error de carga, la tabla enseña el error y un botón de reintento.
- **Reglas de descuento**: Código, Nombre, Tipo (Porcentaje, Importe fijo, Compra X, llévate Y, Por
  tramos), Valor (`10 %`, importe en euros para el fijo, «—» para los otros dos), Prioridad. Sin
  «+»: no hay alta de reglas en pantalla. Vacía: «Sin reglas de descuento.».

No hay pantalla de tramos, de precios por artículo ni de prueba de un cálculo.

## Flujos

### PRICING-F01 Ver las tarifas
Estado: hecho
Actor: administrador, responsable, empleado
Pantalla: Tarifas
Pasos:
1. Abrir «Tarifas» en el menú.
2. Buscar por código o nombre, ordenar o filtrar por columna.
3. Leer, por tarifa, divisa, segmento y base fiscal («Según el hub» si no tiene la suya).
Entra: las tarifas activas y no borradas del negocio (solo las activas: una desactivada no aparece).
Sale: nada.
Si falla: la tabla enseña el error de carga y un botón para reintentar. Sin permiso de ver, el
servidor rechaza la consulta.
Implicados: ninguno
QA: ninguno

### PRICING-F02 Crear una tarifa
Estado: hecho
Actor: administrador, responsable
Pantalla: Tarifas
Pasos:
1. Pulsar «+» en la tabla de listas.
2. Rellenar Código y Nombre (obligatorios), Divisa y Impuesto (Según el hub por defecto).
3. Pulsar «Guardar».
4. La tarifa aparece en la tabla.
Entra: código, nombre, divisa, base fiscal de la persona. «Según el hub» no hereda nada en la práctica: el hub no le pasa su modo de impuestos al cálculo, y una tarifa así responde siempre «incluido» (ver PRICING-F07). El segmento, la tarifa por defecto y las
fechas de validez **no** se piden en pantalla: solo se pueden fijar por el asistente o la API.
Sale: la tarifa guardada (activa) y el aviso `pricing.price_list.created`. Si se marca como por
defecto (solo por API), se baja la marca a la anterior en la misma operación: nunca hay dos.
Si falla: un código repetido en el negocio no crea nada, marca el campo Código con «Ya existe una
tarifa con ese código. Elige otro.» y le da el foco; el código vale también para una tarifa
retirada. Una fecha mal escrita (`YYYY-MM-DD`) se rechaza (por API). Cualquier otro error: «No se
pudo crear la lista». Otro negocio puede usar el mismo código.
Implicados: pendiente
Pendiente de enlazar: taxes — TAXES-F18 (Calcular el impuesto de un importe): la base fiscal de la tarifa decide si el importe lleva el impuesto dentro; este módulo solo la declara, no calcula impuestos
QA: ninguno

### PRICING-F03 Poner el precio de un artículo en una tarifa
Estado: parcial — no hay pantalla: solo con el asistente o la API
Actor: administrador, responsable, asistente
Pantalla: asistente
Pasos:
1. Elegir la tarifa (tiene que existir en este negocio, estar activa y no borrada).
2. Dar la referencia del artículo, el precio en céntimos y, si hay tramos, la cantidad mínima y la
   máxima en millonésimas de unidad (una unidad = 1 000 000); los dos extremos cuentan.
3. Confirmar. El mismo artículo puede repetirse con tramos distintos.
Entra: el artículo se identifica por un texto libre (`product_ref`), sin comprobar que exista en
Inventario o Servicios.
Sale: la fila de precio y el aviso `pricing.price_item.added`.
Si falla: si la tarifa no es de este negocio, no existe o está inactiva, no se escribe nada ni se
avisa, y sale «Esa tarifa no está disponible: no existe en este negocio, o se ha borrado o
desactivado.». No se comprueba que un tramo no se solape con otro ni que el precio sea positivo. Por el asistente, la tarjeta de confirmación dice «Una acción que esta app no sabe nombrar»: el módulo no trae etiqueta de la orden ni nivel de riesgo.
Implicados: pendiente
Pendiente de enlazar: inventory — precio del artículo: la referencia del artículo es un texto que no se valida contra el catálogo y el precio de Inventario no se sincroniza con la tarifa
QA: ninguno

### PRICING-F04 Ver las reglas de descuento
Estado: hecho
Actor: administrador, responsable, empleado
Pantalla: Tarifas
Pasos:
1. Abrir «Tarifas» y bajar a «Reglas de descuento».
2. Buscar, ordenar o filtrar por tipo y prioridad (la columna Valor no se ordena ni se filtra).
Entra: las reglas activas del negocio.
Sale: nada.
Si falla: error de carga con reintento; vacía: «Sin reglas de descuento.».
Implicados: ninguno
QA: ninguno

### PRICING-F05 Crear una regla de descuento o recargo
Estado: parcial — no hay pantalla: solo con el asistente o la API
Actor: administrador, responsable, asistente
Pantalla: asistente
Pasos:
1. Dar código, nombre y tipo (porcentaje, importe fijo, por tramos, compra X llévate Y).
2. Dar el valor: el porcentaje va en `value`; el importe fijo, en céntimos, en `amount_cents`.
3. Opcionalmente importe mínimo y máximo (céntimos), alcance, condiciones, fechas y prioridad
   (el número menor se aplica antes; por defecto 100).
4. Confirmar.
Entra: los datos de la persona. Las condiciones son un texto JSON libre.
Sale: la regla activa y el aviso `pricing.rule.created`.
Si falla: un código repetido en el negocio no crea nada: «Ese código ya está en uso. Elige otro.». Por el asistente, la tarjeta de confirmación dice «Una acción que esta app no sabe nombrar» (sin etiqueta de orden ni nivel de riesgo).
Implicados: pendiente
Pendiente de enlazar: sales — SALES-F14 (Aplicar un descuento a una línea o a la cuenta): Vender no consulta estas reglas; sus descuentos son manuales y los valida Vender, así que una regla creada aquí no cambia ningún tique
QA: ninguno

### PRICING-F06 Retirar una regla de descuento
Estado: parcial — no hay pantalla: solo con el asistente o la API
Actor: administrador, responsable, asistente
Pantalla: asistente
Pasos:
1. Indicar la regla.
2. Confirmar: la regla pasa a inactiva y desaparece de «Reglas de descuento».
Entra: el identificador de la regla.
Sale: la regla inactiva (no se borra; no hay forma de reactivarla) y el aviso
`pricing.rule.deactivated`. Si la regla no existe o es de otro negocio no cambia nada, pero **el
aviso sale igual**.
Si falla: sin permiso de gestionar, el servidor lo rechaza. Por el asistente, la tarjeta de confirmación dice «Una acción que esta app no sabe nombrar» (sin etiqueta de orden ni nivel de riesgo).
Implicados: ninguno
QA: ninguno

### PRICING-F07 Consultar el precio de un artículo
Estado: parcial — el precio llega a quien lo pide, pero sus rechazos llegan como un error genérico y la base fiscal nunca se hereda del hub
Actor: sistema, asistente, empleado
Pantalla: ninguna
Pasos:
1. Quien llama da la referencia del artículo y, si quiere, la cantidad (en millonésimas; por defecto
   una unidad), una tarifa concreta o el segmento del cliente.
2. El módulo toma las tarifas candidatas (la pedida, o todas las activas; con segmento, las de ese
   segmento y las sin segmento).
3. Entre los precios del artículo cuyo tramo contiene la cantidad, devuelve el **más bajo**: no la
   tarifa por defecto ni la primera. Si hay tarifas de divisas distintas, compara los números sin
   convertir (90 USD gana a 100 EUR).
4. Responde precio en céntimos, tarifa, divisa, decimales de la divisa, y base fiscal con su origen:
   la de la tarifa o, si la tarifa dice «Según el hub», siempre «incluido» con origen «por defecto»
   (el hub no le pasa su modo de impuestos: un B2B con precios netos cotizaría como bruto).
   Es una orden, no una consulta: por el asistente pasa por la tarjeta de confirmación y por la API de órdenes.
Entra: referencia, cantidad, tarifa o segmento; las tarifas y precios del negocio.
Sale: solo la respuesta; no escribe nada ni avisa.
Si falla: quien llama recibe un error genérico («no se pudo completar»), sin saber si faltaba tarifa
o precio o si las bases se mezclaban (`no_price_list`, `no_price`, `invalid_id`, `mixed_tax_basis`):
el motivo solo queda en el registro del hub. Una cantidad decimal o menor que 1 la rechaza antes el
esquema (entero, mínimo 1); un «1» pelado es una millonésima de unidad y no encaja en ningún tramo.
La **vigencia por fechas de la tarifa no se evalúa**: una tarifa caducada o aún no vigente sigue
dando precio mientras esté activa. La tarifa por defecto no influye en la elección.
Implicados: pendiente
Pendiente de enlazar: sales — SALES-F09 (Vender a precio libre por departamento): hoy el precio de la línea lo decide Inventario/Servicios o el cajero; Vender no llama a esta consulta
Pendiente de enlazar: customers — segmento del cliente: la consulta recibe el segmento como dato, pero Clientes no guarda una tarifa por cliente ni lo envía
Pendiente de enlazar: services — precio de un servicio: no se consulta
QA: ninguno

### PRICING-F08 Calcular el descuento de un importe y repartirlo
Estado: parcial — el resultado llega a quien lo pide, pero sus rechazos llegan como un error genérico, y el alcance «categoría de artículo» no se aplica
Actor: sistema, asistente, empleado
Pantalla: ninguna
Pasos:
1. Quien llama da un importe en céntimos (o las líneas con su referencia, importe y tipo de
   impuesto) y, si quiere, las reglas a usar (todas las activas, algunas por id o definidas al
   vuelo) y el segmento del cliente.
2. Las reglas se ordenan por prioridad (empate: por código) y se aplican una tras otra sobre el
   importe ya descontado. Una regla se salta si el importe está fuera de su mínimo/máximo o si su
   alcance es «segmento» y el segmento no coincide.
3. Cada descuento se limita entre 0 y el importe restante y se redondea una vez al céntimo
   (mitad hacia arriba).
4. Con líneas, el descuento se reparte entre ellas por el método del resto mayor: la suma de las
   partes es el descuento total, al céntimo, ninguna línea baja de 0, y se agrega por tipo de
   impuesto.
Entra: importe o líneas, reglas del negocio, segmento.
Sale: importe original y final, descuento total, reglas aplicadas, base fiscal y, con líneas, el
reparto por línea y por tipo. No escribe nada ni avisa.
Si falla: quien llama recibe un error genérico («no se pudo completar»): el motivo (`invalid_amount`, `amount_mismatch` por líneas que no suman el importe, `mixed_tax_basis`) solo queda en el registro del hub.
No se evalúan las fechas de validez de la regla, y el alcance «categoría de artículo» no filtra
nada (la regla se aplica como si fuera para todo). «Compra X, llévate Y» toma su cantidad y precio
del texto de condiciones, no de las líneas.
Implicados: pendiente
Pendiente de enlazar: sales — SALES-F14 (Aplicar un descuento a una línea o a la cuenta): si un día Vender llamara a este cálculo, quién gana entre el descuento manual y la regla no está definido; hoy Vender no lo llama
Pendiente de enlazar: taxes — TAXES-F18 (Calcular el impuesto de un importe): el reparto por tipo prepara la base que luego se grava
Pendiente de enlazar: invoice — desglose por tipo de la factura: el agregado por tipo está pensado para ese desglose, hoy no se envía
QA: ninguno

## Cobertura contra la referencia
| Elemento | Estado | Flujo |
|---|---|---|
| Listas de precios con divisa, segmento y base fiscal | hecho | PRICING-F02 |
| Ver listas | hecho | PRICING-F01 |
| Precio por artículo con tramos de cantidad | parcial: sin pantalla | PRICING-F03 |
| Reglas de descuento por prioridad (4 tipos) | parcial: ver sí, alta y baja sin pantalla | PRICING-F04 a F06 |
| Calcular el precio de un artículo | parcial: rechazos genéricos; base del hub nunca heredada; sin vigencia | PRICING-F07 |
| Calcular y repartir un descuento a las líneas | parcial: rechazos genéricos; sin vigencia ni categoría | PRICING-F08 |
| Que Vender cobre el precio de la tarifa | no lo hace (módulo sin consumidor) | — |
| Editar o borrar tarifas, precios y reglas | no lo hace | — |
| Tarifa por cliente | no lo hace (Clientes retiró el descuento por grupo, customers#17) | — |

## Datos: de quién es cada dato
- Propios: tarifas, precios por artículo y reglas de descuento; todos con el negocio en cada fila.
- Lee de otros: nada (ni consultas ni avisos de otros módulos; no escucha ningún aviso).
- Emite: `pricing.price_list.created`, `pricing.price_item.added`, `pricing.rule.created`,
  `pricing.rule.deactivated`. Nadie los escucha hoy.
- Importes en **céntimos enteros** (decimales según la divisa, redondeo mitad hacia arriba, una sola
  vez); cantidades en **millonésimas** de unidad; el porcentaje de una regla es una tasa decimal.
- Datos personales: ninguno propio. Solo las columnas de auditoría (quién creó o cambió cada fila).

## Reglas que no se rompen
- Cada fila lleva el negocio; un precio solo se añade a una tarifa del mismo negocio, viva y activa.
- Un código de tarifa o de regla es único por negocio (lo hace cumplir un índice).
- Una sola tarifa por defecto por negocio (lo hace el alta, no un índice).
- Nunca se mezclan bases fiscales distintas en un mismo cálculo: se rechaza.
- El descuento nunca deja un importe o una línea por debajo de 0 y el reparto conserva la suma.
- Gestionar exige `pricing.manage_pricing`; consultar y calcular, `pricing.view_pricing` y
  `pricing.apply_pricing`.

## Lo que NO hace, a propósito
- No cambia lo que cobra Vender ni intercepta ninguna venta.
- No conoce el tique abierto: cambiar una tarifa no toca ningún tique, porque ningún tique la usa
  (Vender congela el precio de la línea al añadirla).
- No convierte divisas ni bases fiscales.
- No edita ni borra tarifas, precios o reglas (solo desactiva reglas).
- No tiene pantalla para tramos, precios de artículo ni alta de reglas.

## Dudas abiertas
- Si Vender debe llamar a este módulo, cuándo y quién gana entre un descuento manual y una regla:
  no está decidido (`market-decision`).

## Fuentes contrastadas
- `docs/limits.md` dice «No screen for rules»: hay tabla de reglas de solo lectura (sin alta).
- `docs/limits.md` (líneas 5-8) y `WASM-TODO.md` dicen que la respuesta de los cálculos no llega al llamante o que devuelven `unsupported_readonly_handler`: está desfasado, el hub devuelve el resultado del manejador (hub#70) y el código ya los implementa. Lo que no llega son los códigos de rechazo.
- El documento técnico habla de reparto y redondeo; el código no mira `valid_from`/`valid_until` ni
  `product_category` en los cálculos.
