#!/usr/bin/env python3
"""The quantity brackets are BIGINT µ, and the migration does not lose the halves (pricing#22).

Why this file exists: `pricing_price_list_item.min_quantity`/`max_quantity` were the last `REAL`
quantities in the project — `sales` (014), `kitchen` (005), `invoice` (004) and `cart_checkout`
(003) had all migrated to the fixed-point integer of ADR-0147 §2.1 and this module was left behind.

Comparing a `REAL` against an integer of scale 10⁶ does not raise. It answers — a price that is off
by a factor of a million and looks perfectly reasonable on screen. That is why this is worth a
migration on a module nobody uses yet: the day `pricing` gets its first consumer, the same ALTER
would have to run over real customer tariffs, with a backfill.

Three halves, all here:

  1. The TYPE, after every migration: both columns are `bigint`, `min_quantity` defaults to
     1000000 (one unit), and `max_quantity` still accepts NULL — "no upper bound", not "zero".
  2. The CONVERSION, which is the part a type change can silently get wrong: rows written while the
     column was `REAL` must come out re-scaled, and **0.5 must become 500000, not 0**. That is the
     whole reason the migration rounds before casting instead of casting first — `0.5::BIGINT` is
     `0`, which is literally the bug ADR-0147 was written to close.
  3. The INSERT path afterwards: `commands/price_item_add.sql` writes the µ integer it is given and
     an absent `max_quantity` lands as NULL.

Usage: tests/quantity_fixed_point.pg.test.py   (exit 0 = green)
  Uses the `erplora-test-pg-5433` container by default (override: ERPLORA_TEST_PG_CONTAINER).
  Creates a scratch database and DROPS it at the end, pass or fail. If Docker or the container is
  missing everything is SKIPPED, never passed.
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
DB = f"pricing_qty_{uuid.uuid4().hex[:8]}"
HUB = "hub-a"
USER = "admin"
NOW = "2026-08-19T10:00:00+00:00"
MICRO = 1_000_000

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


def run_command(name: str, payload: dict) -> tuple[bool, str, str]:
    cmd = MANIFEST["commands"][name]
    params = dict(payload)
    params.update(hub_id=HUB, current_user_id=USER, now=NOW)
    new_id = str(uuid.uuid4())
    params["new_id"] = new_id
    body = "\n".join(bind((MODULE_DIR / rel).read_text(), params) for rel in cmd["sql"])
    try:
        psql([], db=DB, stdin="BEGIN;\n" + body + "\nCOMMIT;")
        return True, "", new_id
    except RuntimeError as exc:
        return False, str(exc).splitlines()[0], new_id


def column(name: str) -> tuple[str, str]:
    row = q(
        "SELECT data_type || '|' || coalesce(column_default, '') "
        "FROM information_schema.columns "
        f"WHERE table_name = 'pricing_price_list_item' AND column_name = '{name}'"
    )
    kind, _, default = row.partition("|")
    return kind, default


def main() -> int:
    migrations = MANIFEST["migrations"]["postgres"]
    print(
        "· the migration is DECLARED (an undeclared file is a file the hub never runs)"
    )
    check(
        "005_quantity_fixed_point.sql is in module.json",
        True,
        "migrations/postgres/005_quantity_fixed_point.sql" in migrations,
    )
    check(
        "it is the LAST one (migrations are linear)",
        "migrations/postgres/005_quantity_fixed_point.sql",
        migrations[-1],
    )

    if not docker_available():
        print(f"SKIPPED: no Postgres in container {CONTAINER}")
        return 1 if failures else 0

    psql(["-c", f"CREATE DATABASE {DB}"])
    try:
        # ── Apply everything BUT the new migration, then write rows the old way. ──
        for rel in migrations[:-1]:
            psql([], db=DB, stdin=(MODULE_DIR / rel).read_text())

        print("· before the migration the columns were REAL — the premise of the issue")
        kind, _ = column("min_quantity")
        check("min_quantity was REAL", "real", kind)

        psql(
            [],
            db=DB,
            stdin=(
                "INSERT INTO pricing_price_list (id, hub_id, code, name, currency, is_active, is_deleted) "
                f"VALUES ('L1', '{HUB}', 'L1', 'Tarifa', 'EUR', 1, 0);"
                "INSERT INTO pricing_price_list_item "
                "(id, hub_id, price_list_id, product_ref, price, min_quantity, max_quantity, is_deleted) VALUES "
                f"('i-half', '{HUB}', 'L1', 'GAMBAS', 900, 0.5, 2.5, 0),"
                f"('i-one',  '{HUB}', 'L1', 'CAFE',   100, 1,   9,   0),"
                f"('i-open', '{HUB}', 'L1', 'CAFE',    80, 100, NULL, 0),"
                f"('i-thou', '{HUB}', 'L1', 'PALES',  500, 5000, NULL, 0);"
            ),
        )

        # ── The migration under test. ──
        psql([], db=DB, stdin=(MODULE_DIR / migrations[-1]).read_text())

        print("· the TYPE is now the fixed-point integer of ADR-0147")
        kind, default = column("min_quantity")
        check("min_quantity is bigint", "bigint", kind)
        check(
            "its default is ONE UNIT in µ, not a bare 1",
            "1000000",
            default.split("::")[0],
        )
        kind, _ = column("max_quantity")
        check("max_quantity is bigint", "bigint", kind)
        check(
            "max_quantity still accepts NULL (= no upper bound, not zero)",
            "YES",
            q(
                "SELECT is_nullable FROM information_schema.columns "
                "WHERE table_name = 'pricing_price_list_item' AND column_name = 'max_quantity'"
            ),
        )

        print("· THE CONVERSION: half a kilo is 500000, NOT 0")
        row = json.loads(
            q(
                "SELECT row_to_json(i) FROM pricing_price_list_item i WHERE id = 'i-half'"
            )
        )
        check("min 0,5 → 500000", 500_000, row["min_quantity"])
        check("max 2,5 → 2500000", 2_500_000, row["max_quantity"])

        print("· and the whole numbers scale too")
        rows = {
            r["id"]: r
            for r in json.loads(
                q(
                    "SELECT json_agg(row_to_json(i)) FROM pricing_price_list_item i WHERE id <> 'i-half'"
                )
            )
        }
        check("min 1 → 1000000", MICRO, rows["i-one"]["min_quantity"])
        check("max 9 → 9000000", 9 * MICRO, rows["i-one"]["max_quantity"])
        check("min 100 → 100000000", 100 * MICRO, rows["i-open"]["min_quantity"])
        check("an open bracket stays NULL", None, rows["i-open"]["max_quantity"])
        check(
            "5000 units → 5000000000, which does NOT fit in INTEGER (hence BIGINT)",
            5000 * MICRO,
            rows["i-thou"]["min_quantity"],
        )
        check("…and it really is over 2^31", True, 5000 * MICRO > 2**31)

        print("· the INSERT path writes µ and nothing else")
        ok, err, iid = run_command(
            "pricing.price_lists.add_item",
            {
                "price_list_id": "L1",
                "product_ref": "GAMBAS",
                "price": 850,
                "min_quantity": 2_500_000,
                "max_quantity": 10_000_000,
            },
        )
        check("insert accepted", True, ok if ok else err)
        if ok:
            row = json.loads(
                q(
                    f"SELECT row_to_json(i) FROM pricing_price_list_item i WHERE id = '{iid}'"
                )
            )
            check("min_quantity stored verbatim", 2_500_000, row["min_quantity"])
            check("max_quantity stored verbatim", 10_000_000, row["max_quantity"])

        print("· an OPEN bracket (no ceiling) lands as NULL, not as 0")
        ok, err, iid = run_command(
            "pricing.price_lists.add_item",
            {
                "price_list_id": "L1",
                "product_ref": "CAFE",
                "price": 70,
                "min_quantity": 500 * MICRO,
            },
        )
        check("insert accepted", True, ok if ok else err)
        if ok:
            check(
                "max_quantity is NULL",
                "",
                q(
                    f"SELECT coalesce(max_quantity::text, '') FROM pricing_price_list_item WHERE id = '{iid}'"
                ),
            )
    finally:
        subprocess.run(
            ["docker", "exec", CONTAINER, "dropdb", "-U", "postgres", "--force", DB]
        )

    if failures:
        print(f"\n{len(failures)} failure(s):")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("\nOK: the brackets are µ, and the halves survived the change")
    return 0


if __name__ == "__main__":
    sys.exit(main())
