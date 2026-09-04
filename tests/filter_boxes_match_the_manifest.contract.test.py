#!/usr/bin/env python3
"""Every filter box of this module has to mean what it looks like (ERPlora/hub#1182).

A column header carries a promise: a free-text box says «type a piece of it», a dropdown says
«choose one of these». The manifest is what actually happens — `op: "like"` narrows by fragment,
`op: "eq"` demands the whole value, and a column the `list` block never declares is a box that does
**nothing at all**. When the two disagree the user gets no error: the list simply empties, or the
typing is ignored, and there is nothing on screen to explain it.

The runtime is explicit about the silence. `hub/crates/runtime/src/queries.rs`
(`reject_undeclared_params`) keeps the `f_*` namespace OUT of its 422 on purpose, naming hub#1182:
an undeclared `f_<col>` is dropped, not rejected, because turning it into an error would swap «the
filter does nothing» for «the table breaks». What fixes it is declaring the filter here.

WHY THIS MODULE GOT ITS GATE LAST. The hub#1182 sweep read the 27 module repos and could NOT judge
`pricing`: `erp-pricing-lists.ts` drives TWO list queries from ONE file, so a sweep that splits the
whole source on `key: '` attributes every column of both tables to both queries and answers
nonsense in both directions. The sibling copies of this gate (`inventory` inventory#74, `taxes`
taxes#44) carry that same shortcut and are only correct because those modules paint one table per
file. Here the pairing is DISCOVERED properly instead — table by table, through the template — and
the columns are cut on balanced braces, which also closes the last-column overrun the shortcut has
(taxes#54: the final column's chunk runs to the end of the file, so a `filterType: '…'` written in
a comment further down gets attributed to it).

Read by hand, that pairing said the two boxes below were lying, and this gate is what says it out
loud from now on:

  * `pricing.price_lists.list` — `code` and `currency` are painted as free text and were filtered
    with `op: "eq"`: typing `PROMO` on a list coded `PROMO2026` emptied the table.
  * `pricing.rules.list` — same on `code`.

## The rules, and why each one

| The box says | The manifest must say | Because |
|---|---|---|
| `filterType: 'text'` | `op: 'like'` | a free-text box invites a fragment; `eq` empties the list unless the user types the value whole |
| `filterType: 'select'` | `op: 'eq'` | a closed domain is CHOSEN, and the value chosen is exact — `like` would silently match one value inside another |
| `filterType: 'range'` / `'daterange'` | `op: 'range'` | two bounds need the operator that takes two bounds |
| `filterable: true` | the column IS in `list.filters` | otherwise the runtime drops the `f_<col>` parameter and the box does nothing (hub#1182) |
| `sortable: true` | the column IS in `list.sort` | the sort whitelist is a SECOND door: a header outside it does not sort, silently |

A NUMBER is never a `text` box (`priority`). The engine's `like` is a SUBSTRING match and it casts
the column to text to do it (`CAST(sub.<col> AS TEXT) LIKE '%' || … || '%'`,
`hub/crates/runtime/src/queries.rs`), so on a number it runs happily and answers nonsense: `10`
brings back priority `10`, `100` and `110` alike. That leaves a numeric column two honest shapes —
`range`, which the same engine compares against the real type and is what `inventory` paints on
`price`/`stock`, `sales` on `total` and `tables` on `guests_count`; or no filter at all, which is
what pricing#28 chose for `value` once that column mixed units. `priority` is a plain `INTEGER`
whose whole meaning is an order («lower wins»), so it gets the two bounds.

Usage: tests/filter_boxes_match_the_manifest.contract.test.py   (exit 0 = green)
  No Postgres, no Docker: it reads the manifest and the Web Components.
"""

import json
import pathlib
import re
import sys

MODULE_DIR = pathlib.Path(__file__).resolve().parent.parent
MANIFEST = json.loads((MODULE_DIR / "module.json").read_text(encoding="utf-8"))

#: What the manifest has to declare for each kind of box the table paints.
EXPECTED_OP = {
    "text": "like",
    "select": "eq",
    "range": "range",
    "daterange": "range",
}

#: Why each one, in the words the failure message uses.
WHY = {
    "text": "a free-text box invites a FRAGMENT; with `eq` anything short of the whole value empties the list",
    "select": "a closed domain is CHOSEN, so the match is exact; `like` would match the value inside another one",
    "range": "two bounds need the operator that takes two bounds",
    "daterange": "two bounds need the operator that takes two bounds",
}

#: `(query, painted column) -> the filters the screen really writes`. A screen may legitimately
#: paint a box on one key and send another (`sales` paints `created_at` and filters `erp_date`).
#: No table of this module does that today, so the map is empty — it is kept because the day one
#: does, the remap gets DECLARED and checked here, instead of the gate being weakened to let it by.
REMAPPED: dict[tuple[str, str], tuple[str, ...]] = {}

#: How many list tables this module paints today (price lists, discount rules). The floor is the
#: check on the check: if the discovery stops finding them, a broken sweep would pass by knowing
#: nothing. It is what makes the ONE-FILE-TWO-TABLES shape safe to rely on.
TABLES_TODAY = 2

failures: list[str] = []


def fail(msg: str) -> None:
    failures.append(msg)


def balanced_slice(src: str, open_at: int) -> str:
    """The text from the brace at `open_at` to the one that closes it, brace-counted.

    Cutting on the next `key: '` (what the sibling copies do) hands the LAST column of an array
    everything down to the end of the file — methods, templates and docstrings included — so a
    `filterType: '…'` written in a comment below gets read as that column's (taxes#54).
    """
    depth = 0
    for i in range(open_at, len(src)):
        if src[i] == "{":
            depth += 1
        elif src[i] == "}":
            depth -= 1
            if depth == 0:
                return src[open_at : i + 1]
    return src[open_at:]


def column_getters(src: str) -> dict[str, str]:
    """`getter name -> its body`, for every `private get <name>(): DataTableColumn[]`."""
    bodies = {}
    for m in re.finditer(r"get\s+(\w+)\s*\(\s*\)\s*:\s*DataTableColumn\[\]\s*(\{)", src):
        bodies[m.group(1)] = balanced_slice(src, m.start(2))
    return bodies


def tables(src: str) -> list[tuple[str, str]]:
    """`(query, columns getter)` for every `ok-data-table` the component paints.

    Both halves come from the element itself: `.rows=${this.<field>?.rows}` names the controller,
    and the controller was built with its query. One file with two tables therefore pairs each set
    of columns with ITS query, which is the whole reason this gate exists here.
    """
    controllers = dict(
        re.findall(
            r"this\.(\w+)\s*=\s*createListController[^(]*\(\s*erplora\(\)\s*,\s*'([^']+)'", src
        )
    )
    found = []
    # Cut on the tag, not on `>`: the attributes carry arrow functions and generics
    # (`Record<string, unknown>) =>`), so `[^>]*` stops inside the first one and finds nothing.
    for element in re.split(r"<ok-data-table\b", src)[1:]:
        element = element.split("</ok-data-table>")[0]
        getter = re.search(r"\.columns=\$\{this\.(\w+)\}", element)
        field = re.search(r"\.rows=\$\{this\.(\w+)\??\.rows", element)
        if not getter or not field:
            continue
        query = controllers.get(field.group(1))
        if query is None:
            # A client-side table (no controller) paints no `f_*`: nothing to promise.
            continue
        found.append((query, getter.group(1)))
    return found


def declared_columns(body: str):
    """`(column, filterType|None, filterable, sortable)` for every column the getter returns."""
    out = []
    for m in re.finditer(r"key: '([^']+)'", body):
        # The column object is the innermost `{` that is still open at this `key:`.
        start = body.rfind("{", 0, m.start())
        chunk = balanced_slice(body, start) if start != -1 else body[m.start() :]
        kind = re.search(r"filterType: '(\w+)'", chunk)
        out.append(
            (
                m.group(1),
                kind.group(1) if kind else None,
                "filterable: true" in chunk,
                "sortable: true" in chunk,
            )
        )
    return out


def check(screen: pathlib.Path, query: str, columns) -> None:
    spec = (MANIFEST.get("queries") or {}).get(query)
    if spec is None:
        fail(f"{screen} drives `{query}`, which the manifest does not declare")
        return
    block = spec.get("list") or {}
    if not block:
        fail(f"`{query}` has no `list` block, but {screen} paginates it")
        return
    filters = block.get("filters") or {}
    sortable_whitelist = set(block.get("sort") or [])

    for column, kind, filterable, sortable in columns:
        remap = REMAPPED.get((query, column))
        if remap:
            # The box does not feed its own key: check the columns it really writes instead.
            for target in remap:
                if target not in filters:
                    fail(
                        f"{screen} routes the `{column}` box to `{target}`, which `{query}` does not "
                        f"declare as a filter: the runtime drops `f_{target}` and that choice does nothing"
                    )
        elif filterable and column not in filters:
            fail(
                f"{screen} paints a filter box on `{column}` but `{query}` declares no filter for it: "
                f"the runtime drops the parameter and the box does nothing (hub#1182)"
            )
        elif filterable and kind is None:
            fail(
                f"{screen} paints `{column}` as filterable without a `filterType`: the table cannot "
                f"know what box to draw, and this gate cannot know what `{query}` should promise"
            )
        elif kind:
            op = (filters.get(column) or {}).get("op")
            expected = EXPECTED_OP.get(kind)
            if expected is None:
                fail(
                    f"{screen} paints `{column}` as `filterType: '{kind}'`, which this gate does not know — teach it"
                )
            elif op != expected:
                fail(
                    f"{screen} paints `{column}` as `filterType: '{kind}'` but `{query}` filters it with "
                    f"`op: {op!r}` (expected `{expected}`) — {WHY[kind]}"
                )
        if sortable and column not in sortable_whitelist:
            fail(
                f"{screen} paints `{column}` as sortable but `{query}` does not whitelist it in `list.sort`: "
                f"clicking that header does nothing"
            )


def main() -> int:
    seen = []
    for path in sorted((MODULE_DIR / "ui/components").rglob("*.ts")):
        if path.name.endswith(".test.ts"):
            continue
        src = path.read_text(encoding="utf-8")
        getters = column_getters(src)
        for query, getter in tables(src):
            body = getters.get(getter)
            if body is None:
                fail(
                    f"{path.relative_to(MODULE_DIR)} paints `{query}` with `this.{getter}`, which is not a "
                    f"`DataTableColumn[]` getter of this component: the gate cannot read that table's columns"
                )
                continue
            seen.append((path.relative_to(MODULE_DIR), query, getter))
            check(path.relative_to(MODULE_DIR), query, declared_columns(body))

    if len(seen) < TABLES_TODAY:
        print(
            f"FAIL: only {len(seen)} list table(s) discovered; this module paints at least "
            f"{TABLES_TODAY} (price lists, discount rules). The discovery is broken, and a broken "
            "sweep passes."
        )
        return 1

    if failures:
        print(f"FAIL ({len(failures)}):")
        for f in failures:
            print(f"  - {f}")
        return 1

    covered = ", ".join(sorted({q for _, q, _ in seen}))
    print(
        f"OK: every filter box and every sortable header of {len(seen)} table(s) matches what "
        f"the manifest concedes ({covered})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
