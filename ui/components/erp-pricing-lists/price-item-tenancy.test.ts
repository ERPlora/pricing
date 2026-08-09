// Adding an item to a price list must not be able to point at ANOTHER hub's list (pricing#10).
//
// `pricing.price_lists.add_item` is a declarative command: the runtime injects `:hub_id`, and the
// SQL inserted whatever `:price_list_id` arrived, with no check that the parent list belongs to
// the caller's hub. So a caller holding an id from hub B created a row **in hub A** pointing at a
// list of hub B — a relationship that no screen can show and no report can reconcile. The FK did
// not stop it either: it referenced `pricing_price_list (id)` alone, without the hub.
//
// This file pins the module's half of the guard: the SQL only inserts when the parent is the
// caller's, alive and active; and the command declares `expect_rows`, so a rejected insert rolls
// the whole transaction back and surfaces a domain error instead of reporting success and emitting
// `pricing.price_item.added` over nothing.
//
// The end-to-end proof — two hubs, the neighbour ALIVE, going through the dispatcher — belongs to
// the hub's e2e, which is the door that actually enforces this.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const ROOT = join(__dirname, '../../..');
const manifest = JSON.parse(readFileSync(join(ROOT, 'module.json'), 'utf8')) as {
  id: string;
  commands: Record<string, {
    sql?: string[];
    transaction?: boolean;
    expect_rows?: { op: string; n: number; error: string; message?: string };
  }>;
};

const ADD_ITEM = 'pricing.price_lists.add_item';
const command = () => manifest.commands[ADD_ITEM];

/** The SQL a command runs, without its `--` comments. */
const sqlOf = (file: string) =>
  readFileSync(join(ROOT, file), 'utf8')
    .split('\n')
    .filter((line) => !line.trim().startsWith('--'))
    .join('\n');

const addItemSql = () => (command().sql ?? []).map(sqlOf).join('\n');

describe('the parent price list has to be the caller\'s own', () => {
  it('the insert is conditioned on a parent that belongs to :hub_id', () => {
    const sql = addItemSql();
    expect(sql, 'the insert does not look the parent list up at all').toMatch(/pricing_price_list\b/);
    expect(
      /WHERE\s+EXISTS|SELECT[\s\S]*FROM\s+pricing_price_list/i.test(sql),
      'the insert is unconditional: any price_list_id gets written',
    ).toBe(true);
    // The hub of the parent must be compared against the injected one, not against a payload field.
    expect(sql, 'the parent is not scoped by the hub of the caller').toMatch(/hub_id\s*=\s*:hub_id/);
  });

  it('a deleted or inactive list does not take new items', () => {
    const sql = addItemSql();
    expect(sql).toMatch(/is_deleted\s*=\s*0/);
    expect(sql).toMatch(/is_active\s*=\s*1/);
  });

  it('runs in a transaction, so a rejected insert leaves nothing behind', () => {
    expect(command().transaction).toBe(true);
  });
});

describe('a rejected insert FAILS — it does not report success over nothing', () => {
  it('declares the expect_rows gate', () => {
    const gate = command().expect_rows;
    expect(gate, 'without expect_rows a no-op still commits and emits `price_item.added`').toBeTruthy();
    expect(gate!.op).toBe('min');
    expect(gate!.n).toBeGreaterThanOrEqual(1);
  });

  it('the error code lives in this module\'s namespace, as the installer demands', () => {
    const code = manifest.commands[ADD_ITEM].expect_rows!.error;
    expect(code).toMatch(/^[a-z][a-z0-9_]*\.[a-z][a-z0-9_]*$/);
    expect(code.split('.')[0], 'the installer rejects a code outside the module namespace').toBe(manifest.id);
  });

  it('carries a human fallback, because no shell translates these codes yet', () => {
    const gate = manifest.commands[ADD_ITEM].expect_rows!;
    expect(gate.message, 'without a message the user gets a bare code').toBeTruthy();
    expect(gate.message!.length).toBeLessThanOrEqual(500);
  });
});

describe('the schema stops what the command misses', () => {
  const migrations = readFileSync(join(ROOT, 'migrations/postgres/004_price_list_item_tenant_fk.sql'), 'utf8');

  it('the parent is unique BY HUB, which is what a composite FK can point at', () => {
    expect(migrations).toMatch(/UNIQUE\s*\(\s*hub_id\s*,\s*id\s*\)/i);
  });

  it('the item FK carries the hub, so the database itself refuses a cross-tenant parent', () => {
    expect(migrations).toMatch(/FOREIGN KEY\s*\(\s*hub_id\s*,\s*price_list_id\s*\)/i);
    expect(migrations).toMatch(/REFERENCES\s+pricing_price_list\s*\(\s*hub_id\s*,\s*id\s*\)/i);
  });

  it('is added NOT VALID: it closes the door forward without deleting legacy rows', () => {
    // Validating in place would fail on any pre-existing cross-tenant row and turn a security fix
    // into a destructive migration. `NOT VALID` enforces every NEW row; validating the old ones is
    // a deliberate operation, not a side effect of installing an update.
    expect(migrations).toMatch(/NOT VALID/i);
  });
});
