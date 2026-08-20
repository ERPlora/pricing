#!/usr/bin/env python3
"""A repeated code is a DOMAIN error, not the driver's exception on the shop owner's screen (pricing#29).

Creating a tariff with a code that already existed printed this, in red, at the top of the screen:

    db: sqlx: error returned from database: duplicate key value violates unique constraint
    "uq_pricing_list_hub_code" at line 666

Two separate defects in one message:

  1. NOBODY CAN ACT ON IT. A shop owner has no way to know this means "you already have a tariff
     with the code PVP". The panel stayed open with the data typed in and nothing pointed at the
     offending field, so it could not even be deduced by elimination.
  2. IT LEAKS THE INSIDES. The engine and its access layer (`sqlx`), the index name and an internal
     line number crossed the wire. None of that belongs outside the server.

Underneath both: `"code": "error"` — no stable code, so the UI could not tell this case apart from
any other failure even if it wanted to.

## The two code paths need DIFFERENT mechanisms, and that is the crux

The issue proposed `ON CONFLICT DO NOTHING` + `expect_rows` for both. That is right for one of them
and would have been ACTIVELY HARMFUL for the other:

  · `pricing.rules.create` is **Tier 0** (declarative `sql`). The runtime runs it through
    `execute_tx_gated`, which is what applies `expect_rows` → `RuntimeError::Domain` → HTTP 409
    with a stable code. `ON CONFLICT (hub_id, code) DO NOTHING` turns the collision into "zero rows
    written" so the gate gets a chance to run at all. ✅ the issue's proposal, as written.

  · `pricing.price_lists.create` is **Tier 2** (WASM handler emitting `pricing._insert_price_list`).
    Until hub#1071 (merged to `develop` on 2026-08-20) the WASM path called `db.execute_tx` — not
    `execute_tx_gated` — so `expect_rows` on a sub-command reached through a handler was NEVER
    applied (hub#1025). Since #1071 it is, with a per-operation gate.

    That fix is not in production yet: the fleet runs images pinned to TAGS. A guard that only
    works on the new runtime fixes nobody's hub today. And adding `ON CONFLICT DO NOTHING` on its
    own would have turned a loud error into a SILENT NO-OP on the runtime everyone is actually
    running: the panel closes, no error, and no tariff. Strictly worse than the bug being fixed.

    So this path uses the documented Tier-2 mechanism instead (hub#139 / ADR-0205, the same one
    `inventory.stock.decrease` uses for `insufficient_stock`): the host pre-loads a read, the
    handler sees the collision and returns `Output.error`, and the host aborts before applying
    anything. It works on BOTH runtimes, and where it matters it is the better of the two anyway:
    it rejects before attempting any write, and it can name the clashing code — which a fixed
    `expect_rows.message` in the manifest cannot. Its SQL therefore keeps NO `ON CONFLICT`: in the
    rare race that beats the read, the unique index must still refuse the write rather than
    swallow it.

## Why the read is a dedicated query and not `price_lists.list`

Two traps, both silent:

  · `pricing.price_lists.list` declares a `list` block, and a `reads` on a paginated query gets
    only the FIRST PAGE (hub#650). Past 50 tariffs the check would start missing duplicates.
  · it filters `is_active = 1 AND is_deleted = 0`, but `uq_pricing_list_hub_code` covers
    `(hub_id, code)` with NO such filter — a deactivated tariff still owns its code. The check has
    to match the INDEX, not a business view of it.

Hence `pricing.price_lists.code_taken`: no `list` block, no status filter, `required: true` so a
read that cannot resolve ABORTS instead of degrading into a false "no duplicate".

## Layers

  1. MANIFEST — each path declares its own mechanism, and the codes are module-namespaced.
  2. SQL — Tier 0 carries `ON CONFLICT (hub_id, code)`; the Tier 2 insert does NOT.
  3. NO LEAKS — no declared message names `sqlx`, `constraint`, `db:` or an index.
  4. POSTGRES — against a scratch DB from this module's own migrations: the Tier-0 duplicate must
     not raise and must write ZERO rows (what `expect_rows` turns into the 409), while still
     letting another hub and another code through. The Tier-2 half is proved by the handler's own
     Rust tests (`cargo test -p pricing-handler`), which is where that logic lives.

Usage: tests/duplicate_code.pg.test.py   (exit 0 = green)
  Uses the `erplora-test-pg-5433` container by default (override: ERPLORA_TEST_PG_CONTAINER).
  Creates a scratch database and DROPS it at the end, pass or fail. If Docker or the container is
  missing, layers 1-3 still run and layer 4 is SKIPPED — never passed.
"""

import json
import os
import pathlib
import re
import subprocess
import sys
import uuid

MODULE_DIR = pathlib.Path(__file__).resolve().parent.parent
MANIFEST = json.loads((MODULE_DIR / "module.json").read_text())
CONTAINER = os.environ.get("ERPLORA_TEST_PG_CONTAINER", "erplora-test-pg-5433")
DB = f"pricing_dupe_{uuid.uuid4().hex[:8]}"
HUB = "hub-a"
OTHER_HUB = "hub-b"
USER = "admin"
NOW = "2026-08-19T10:00:00+00:00"

# The stable code both paths must answer with.
DUPLICATE_CODE = "pricing.duplicate_code"
# The read that mirrors the unique index for the Tier 2 path.
CODE_TAKEN_QUERY = "pricing.price_lists.code_taken"

# Strings that must never reach a user-facing message.
LEAKS = ["sqlx", "constraint", "db:", "uq_pricing"]

failures: list[str] = []


def check(label: str, expected, actual):
    if expected != actual:
        failures.append(f"{label} — expected [{expected}], got [{actual}]")
        print(f"  FAIL: {label} — expected [{expected}], got [{actual}]")
    else:
        print(f"  ok: {label} = {expected}")


def docker_available() -> bool:
    try:
        r = subprocess.run(
            ["docker", "exec", CONTAINER, "pg_isready", "-U", "postgres"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        return r.returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


def psql(args: list[str], db: str | None = None, stdin: str | None = None) -> str:
    cmd = [
        "docker",
        "exec",
        "-i",
        CONTAINER,
        "psql",
        "-v",
        "ON_ERROR_STOP=1",
        "-U",
        "postgres",
        "-X",
    ]
    if db:
        cmd += ["-d", db]
    cmd += args
    res = subprocess.run(cmd, input=stdin, capture_output=True, text=True)
    if res.returncode != 0:
        raise RuntimeError(res.stderr.strip() or res.stdout.strip())
    return res.stdout


def q(sql: str) -> str:
    return psql(["-tAc", sql], db=DB).strip()


PARAM = re.compile(r":([a-z_][a-z0-9_]*)", re.IGNORECASE)


def literal(value) -> str:
    if value is None:
        return "NULL"
    if isinstance(value, bool):
        return "1" if value else "0"
    if isinstance(value, (int, float)):
        return str(value)
    return "'" + str(value).replace("'", "''") + "'"


def bind(sql: str, params: dict) -> str:
    """Replace `:param` with a literal, leaving anything inside a `--` comment alone."""
    spans, i, n = [], 0, len(sql)
    while i < n:
        if sql.startswith("--", i):
            j = sql.find("\n", i)
            j = n if j < 0 else j
            spans.append((i, j))
            i = j
        else:
            i += 1

    def in_comment(pos: int) -> bool:
        return any(a <= pos < b for a, b in spans)

    return PARAM.sub(
        lambda m: (
            m.group(0) if in_comment(m.start()) else literal(params.get(m.group(1)))
        ),
        sql,
    )


def sql_of(cmd_name: str) -> str:
    cmd = MANIFEST["commands"].get(cmd_name, {})
    return "\n".join((MODULE_DIR / rel).read_text() for rel in cmd.get("sql", []))


def run_command(name: str, payload: dict, hub: str = HUB) -> tuple[bool, str, int]:
    """Run a command's SQL like the runtime would. Returns (ok, first error line, rows written).

    The row count is the point: `expect_rows` is applied by the runtime, not by psql, so what this
    proves is the CONDITION the runtime acts on — a duplicate must reach it as zero rows written
    rather than as a thrown exception.
    """
    cmd = MANIFEST["commands"][name]
    params = dict(payload)
    params.update(hub_id=hub, current_user_id=USER, now=NOW)
    params.setdefault("new_id", str(uuid.uuid4()))
    params.setdefault("price_list_id", params["new_id"])
    body = "\n".join(bind((MODULE_DIR / rel).read_text(), params) for rel in cmd["sql"])
    try:
        out = psql([], db=DB, stdin="BEGIN;\n" + body + "\nCOMMIT;")
        written = sum(
            int(m) for m in re.findall(r"^INSERT \d+ (\d+)$", out, re.MULTILINE)
        )
        return True, "", written
    except RuntimeError as exc:
        return False, str(exc).splitlines()[0], 0


def rule_payload(code: str, name: str) -> dict:
    return {
        "code": code,
        "name": name,
        "rule_type": "percent",
        "value": 10,
        "amount_cents": 0,
        "min_amount": None,
        "max_amount": None,
        # NOT NULL with a table default; the JSON Schema fills both in before the SQL ever runs,
        # so a NULL here would be testing a payload the dispatcher never produces.
        "applies_to": "all",
        "conditions": "{}",
        "valid_from": None,
        "valid_until": None,
        "priority": 100,
    }


def main() -> int:
    commands = MANIFEST["commands"]
    queries = MANIFEST["queries"]

    # ── 1a. TIER 0 (`pricing.rules.create`): expect_rows is the mechanism ──────────────────────
    print("· 1a. Tier 0 — the declarative gate turns zero rows into a 409")
    rules = commands.get("pricing.rules.create", {})
    check(
        "pricing.rules.create is still declarative (no WASM handler)",
        False,
        "handler" in rules,
    )
    expect = rules.get("expect_rows")
    check("it declares expect_rows", True, isinstance(expect, dict))
    if isinstance(expect, dict):
        check("expect_rows.op", "min", expect.get("op"))
        check("expect_rows.n >= 1", True, int(expect.get("n", 0)) >= 1)
        check(
            "expect_rows.error is the stable duplicate code",
            DUPLICATE_CODE,
            expect.get("error"),
        )
        check("expect_rows has a human message", True, bool(expect.get("message")))

    # ── 1b. TIER 2 (`pricing.price_lists.create`): a read + the handler's domain error ─────────
    print(
        "\n· 1b. Tier 2 — the handler rejects first (works on the pinned runtime, and names the code)"
    )
    create = commands.get("pricing.price_lists.create", {})
    check("pricing.price_lists.create is a WASM handler", True, "handler" in create)
    check(
        # Not "because expect_rows would not run" any more (hub#1071 fixed that in `develop`), but
        # because the handler check works on the pinned runtime too AND names the clashing code.
        "it does NOT rely on expect_rows (the handler rejects first, on any runtime version)",
        False,
        "expect_rows" in create,
    )
    reads = create.get("reads", [])
    entry = next(
        (
            r
            for r in reads
            if isinstance(r, dict) and r.get("query") == CODE_TAKEN_QUERY
        ),
        None,
    )
    check(f"it pre-loads {CODE_TAKEN_QUERY}", True, entry is not None)
    if entry:
        check(
            "filtered by the code being created",
            "payload.code",
            entry.get("params", {}).get("code"),
        )
        check(
            "and REQUIRED — a read that cannot resolve must abort, not degrade to «no duplicate»",
            True,
            entry.get("required") is True,
        )

    print(
        f"\n· 1c. {CODE_TAKEN_QUERY} mirrors the unique index, not a business view of it"
    )
    taken = queries.get(CODE_TAKEN_QUERY)
    check("the query exists", True, taken is not None)
    if taken:
        check(
            "no `list` block — a `reads` on a paginated query gets only the first page (hub#650)",
            False,
            "list" in taken,
        )
        body = re.sub(r"--[^\n]*", " ", (MODULE_DIR / taken["sql"]).read_text())
        check(
            "it filters by hub_id",
            True,
            bool(re.search(r"hub_id\s*=\s*:hub_id", body, re.I)),
        )
        check(
            "it filters by code",
            True,
            bool(re.search(r"\bcode\s*=\s*:code", body, re.I)),
        )
        check(
            "it does NOT filter by is_active/is_deleted — the index does not, and a retired "
            "tariff still owns its code",
            False,
            bool(re.search(r"is_active|is_deleted", body, re.I)),
        )

    # ── 2. SQL: ON CONFLICT belongs to the gated path ONLY ────────────────────────────────────
    print("\n· 2. ON CONFLICT only where a gate can act on it")
    rules_sql = re.sub(r"--[^\n]*", " ", sql_of("pricing.rules.create"))
    check(
        "rules.create absorbs the collision (without it the driver throws before expect_rows)",
        True,
        bool(
            re.search(r"ON\s+CONFLICT\s*\(\s*hub_id\s*,\s*code\s*\)", rules_sql, re.I)
        ),
    )
    insert_sql = re.sub(r"--[^\n]*", " ", sql_of("pricing._insert_price_list"))
    check(
        "_insert_price_list does NOT — a silent DO NOTHING there would close the panel and save "
        "nothing, which is worse than the error being fixed",
        False,
        bool(re.search(r"\bON\s+CONFLICT\b", insert_sql, re.I)),
    )

    # ── 3. NO LEAKS ───────────────────────────────────────────────────────────────────────────
    print("\n· 3. no declared message leaks the driver, the engine or the index name")
    messages = [str(rules.get("expect_rows", {}).get("message", ""))]
    handler_src = (MODULE_DIR / "handler/src/lib.rs").read_text()
    for m in re.findall(r'DomainError::new\(\s*[^,]+,\s*"([^"]*)"', handler_src):
        messages.append(m)
    check("there IS a message to inspect", True, any(messages))
    for msg in messages:
        for leak in LEAKS:
            if leak in msg.lower():
                check(f"message does not contain `{leak}`", False, True)
    print(f"  ok: {len(messages)} message(s) inspected, none leaks {LEAKS}")

    if not docker_available():
        print(f"\nSKIPPED (layer 4): no Postgres in container {CONTAINER}")
        return 1 if failures else 0

    # ── 4. POSTGRES: the Tier 0 proof ─────────────────────────────────────────────────────────
    print(f"\n· 4. pricing.rules.create against a scratch database ({DB})")
    subprocess.run(
        ["docker", "exec", CONTAINER, "createdb", "-U", "postgres", DB], check=True
    )
    try:
        for rel in MANIFEST["migrations"]["postgres"]:
            psql([], db=DB, stdin=(MODULE_DIR / rel).read_text())

        ok, err, written = run_command(
            "pricing.rules.create", rule_payload("PVP", "10% general")
        )
        check("the first rule is created", True, ok if ok else err)
        check("and it writes its row", 1, written)

        ok, err, written = run_command(
            "pricing.rules.create", rule_payload("PVP", "Duplicada QA")
        )
        # THE regression: before the fix this raised, and the exception text reached the screen.
        check(
            "the DUPLICATE does not raise the driver's exception",
            True,
            ok if ok else err,
        )
        check(
            "it writes ZERO rows — which is what expect_rows turns into the 409",
            0,
            written,
        )
        check(
            "and the table still holds exactly one rule with that code",
            "1",
            q(
                f"SELECT count(*) FROM pricing_discount_rule WHERE hub_id = '{HUB}' AND code = 'PVP'"
            ),
        )

        # The index is per hub: ON CONFLICT must not widen uniqueness to the whole table, which
        # would leak one tenant's codes into another's namespace.
        ok, err, written = run_command(
            "pricing.rules.create", rule_payload("PVP", "Otro hub"), hub=OTHER_HUB
        )
        check(
            "another hub can still use the same code (tenancy is per hub)",
            True,
            ok if ok else err,
        )
        check("and its row IS written", 1, written)

        # Without this, an INSERT that always wrote zero rows would pass every check above.
        ok, err, written = run_command(
            "pricing.rules.create", rule_payload("PVP2", "Otra regla")
        )
        check(
            "a DIFFERENT code in the same hub is still created", True, ok if ok else err
        )
        check("and its row IS written", 1, written)
    finally:
        subprocess.run(
            ["docker", "exec", CONTAINER, "dropdb", "-U", "postgres", "--force", DB]
        )

    if failures:
        print(f"\n{len(failures)} failure(s):")
        for f in failures:
            print(f"  - {f}")
        return 1
    print(
        "\nOK: a repeated code is a 409 with a stable code, and the driver stays on the server"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
