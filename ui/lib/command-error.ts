// command-error — lo que el usuario lee cuando un alta se rechaza (pricing#29).
//
// Antes se pintaba `e.message` tal cual, así que un código repetido acababa en pantalla como:
//
//     db: sqlx: error returned from database: duplicate key value violates unique constraint
//     "uq_pricing_list_hub_code" at line 666
//
// Dos cosas mal: el dueño de la tienda no puede actuar sobre eso, y el mensaje publica el motor, su
// capa de acceso, el nombre del índice y un número de línea interno.
//
// El servidor ya devuelve un código estable (`pricing.duplicate_code`, HTTP 409). Aquí se traduce
// ESE CÓDIGO —nunca se parsea el texto— y se dice DE QUÉ CAMPO cuelga, para que el formulario
// marque el que hay que corregir en vez de soltar una banda roja arriba del todo.
//
// La última red: pase lo que pase, el texto del driver NO sale a pantalla. Aunque el servidor
// mande un `db: sqlx: …` (un fallo que todavía no tenga su código de dominio), aquí se sustituye
// por el mensaje genérico del módulo. Es la lección de la propia issue: el camino por el que se
// coló el error crudo era justamente el del «error desconocido».

/** Código estable que el servidor devuelve cuando el código de tarifa/regla ya existe. */
export const DUPLICATE_CODE = 'pricing.duplicate_code';

/** Códigos del módulo → clave i18n de su mensaje, y el campo del formulario que los provoca. */
const BY_CODE: Record<string, { key: string; field: string | null }> = {
  [DUPLICATE_CODE]: { key: 'ui.errDuplicateCode', field: 'code' },
};

/** Clave i18n del «no se pudo, y no sabemos decir más». Nunca se enseña el error del servidor. */
const GENERIC_KEY = 'ui.createListError';

/** Marcas de que un texto viene de las tripas y no es presentable a un usuario. */
const INTERNALS = ['sqlx', 'constraint', 'db:', 'uq_pricing', 'at line '];

/** Lo que el formulario necesita saber para pintar el fallo. */
export interface CommandError {
  /** Texto ya traducido y presentable. Nunca vacío, nunca con tripas dentro. */
  message: string;
  /** Campo del formulario al que colgarlo (`'code'`), o `null` si no es de un campo concreto. */
  field: string | null;
}

/** ¿Este texto es enseñable, o son las tripas del servidor? */
function presentable(text: string): boolean {
  const t = text.trim().toLowerCase();
  return t.length > 0 && !INTERNALS.some((mark) => t.includes(mark));
}

/**
 * Traduce el rechazo de un command a algo que el usuario pueda leer y corregir.
 *
 * @param e   lo que capturó el `catch` (un `ErploraError` con `code`, un `Error` pelado, o
 *            cualquier cosa: un `catch` no garantiza el tipo).
 * @param t   traductor del módulo, ya atado a su catálogo.
 */
export function commandError(e: unknown, t: (key: string) => string): CommandError {
  const code = typeof e === 'object' && e !== null ? String((e as { code?: unknown }).code ?? '') : '';
  const serverMessage = e instanceof Error ? e.message : '';
  const known = BY_CODE[code];

  if (known) {
    const translated = t(known.key);
    // Un catálogo sin la entrada devuelve la clave cruda: `ui.errDuplicateCode` no es un mensaje.
    if (translated && translated !== known.key) return { message: translated, field: known.field };
    // Respaldo: el mensaje del servidor, pero SOLO si es presentable.
    if (presentable(serverMessage)) return { message: serverMessage, field: known.field };
    return { message: fallback(t), field: known.field };
  }

  return { message: fallback(t), field: null };
}

/** El genérico del módulo, con un último respaldo por si el catálogo tampoco lo tiene. */
function fallback(t: (key: string) => string): string {
  const generic = t(GENERIC_KEY);
  return generic && generic !== GENERIC_KEY ? generic : 'The operation could not be completed.';
}
