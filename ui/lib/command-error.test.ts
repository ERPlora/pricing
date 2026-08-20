// Contrato del ERROR que ve el usuario al dar de alta (pricing#29).
//
// Un código repetido escupía el error de PostgreSQL en crudo, en rojo, arriba del listado:
//
//     db: sqlx: error returned from database: duplicate key value violates unique constraint
//     "uq_pricing_list_hub_code" at line 666
//
// El dueño de una tienda no tiene forma de saber que eso significa «ya tienes una tarifa con el
// código PVP», y de paso el mensaje publicaba el motor, su capa de acceso, el nombre del índice y
// un número de línea interno.
//
// El servidor ya devuelve ahora un código estable (`pricing.duplicate_code`, HTTP 409). Esto es la
// otra mitad: traducirlo al idioma del hub y colgarlo DEL CAMPO que lo provoca, no de una banda
// suelta arriba del todo.

import { describe, expect, it } from 'vitest';
import { commandError, DUPLICATE_CODE } from './command-error';

/** `t` de mentira: devuelve la clave (lo que hace el catálogo cuando le falta la entrada). */
const rawT = (k: string): string => k;
/** `t` con las entradas que existen de verdad. */
const es = (k: string): string =>
  k === 'ui.errDuplicateCode'
    ? 'Ya existe una tarifa con ese código. Elige otro.'
    : k === 'ui.createListError'
      ? 'No se pudo crear la tarifa.'
      : k;

/** Lo que lanza el SDK: un `ErploraError` con `code` estable. */
function erploraError(code: string, message: string): Error {
  return Object.assign(new Error(message), { code, name: 'ErploraError' });
}

const RAW_PG =
  'db: sqlx: error returned from database: duplicate key value violates unique constraint ' +
  '"uq_pricing_list_hub_code" at line 666';

describe('un código repetido se explica en el idioma del hub (pricing#29)', () => {
  it('lo traduce por su CÓDIGO estable, no parseando el mensaje', () => {
    const e = commandError(erploraError(DUPLICATE_CODE, 'A price list with the code…'), es);
    expect(e.message).toBe('Ya existe una tarifa con ese código. Elige otro.');
  });

  it('lo cuelga del campo CÓDIGO, que es el que hay que corregir', () => {
    const e = commandError(erploraError(DUPLICATE_CODE, 'whatever'), es);
    expect(e.field).toBe('code');
  });

  it('NUNCA deja pasar el texto del driver, aunque el servidor lo mande', () => {
    const e = commandError(erploraError(DUPLICATE_CODE, RAW_PG), es);
    for (const leak of ['sqlx', 'constraint', 'uq_pricing', 'db:', '666']) {
      expect(e.message, `se filtró «${leak}»`).not.toContain(leak);
    }
  });
});

describe('cualquier OTRO fallo tampoco publica las tripas del servidor', () => {
  it('un error sin código conocido cae al mensaje genérico del módulo', () => {
    const e = commandError(erploraError('error', RAW_PG), es);
    expect(e.message).toBe('No se pudo crear la tarifa.');
    expect(e.field).toBeNull();
  });

  it('y ese camino es justo por donde se colaba el error crudo', () => {
    const e = commandError(erploraError('error', RAW_PG), es);
    for (const leak of ['sqlx', 'constraint', 'uq_pricing', 'db:', '666']) {
      expect(e.message, `se filtró «${leak}»`).not.toContain(leak);
    }
  });

  it('un Error pelado (fallo de red, sin `code`) también', () => {
    const e = commandError(new Error('TypeError: failed to fetch'), es);
    expect(e.message).toBe('No se pudo crear la tarifa.');
    expect(e.field).toBeNull();
  });

  it('algo que ni siquiera es un Error no rompe nada', () => {
    const e = commandError('boom', es);
    expect(e.message).toBe('No se pudo crear la tarifa.');
    expect(e.field).toBeNull();
  });
});

describe('degradación del catálogo i18n', () => {
  it('si falta la traducción NO se enseña `ui.errDuplicateCode` al usuario', () => {
    const e = commandError(erploraError(DUPLICATE_CODE, 'A price list with that code exists'), rawT);
    expect(e.message).not.toContain('ui.err');
    // Cae al mensaje del servidor, que ya es humano y viene sin tripas.
    expect(e.message).toBe('A price list with that code exists');
  });

  it('y si TAMPOCO hay mensaje del servidor, nunca queda vacío', () => {
    const e = commandError(erploraError(DUPLICATE_CODE, ''), rawT);
    expect(e.message.length).toBeGreaterThan(0);
    expect(e.message).not.toContain('ui.err');
  });

  it('un mensaje de servidor CON tripas no se usa como respaldo', () => {
    // El respaldo solo vale si es presentable; si trae `sqlx`/`constraint`, no lo es.
    const e = commandError(erploraError(DUPLICATE_CODE, RAW_PG), rawT);
    for (const leak of ['sqlx', 'constraint', 'uq_pricing']) {
      expect(e.message, `se filtró «${leak}»`).not.toContain(leak);
    }
  });
});
