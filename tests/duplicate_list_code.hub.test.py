#!/usr/bin/env python3
"""A repeated price-list code is refused with its DOMAIN code, per hub — against the real kernel
(pricing#46).

pricing#46 reported the create panel rejecting a repeated code «in silence». The screen half lives
in `erp-pricing-lists.test.ts` (the «Code» field takes the focus so its message is in view). This
battery pins the half the screen depends on, through the REAL door — the dispatcher pre-loading the
`pricing.price_lists.code_taken` read before the WASM handler runs, and injecting `hub_id`:

  §1 the first create of a code succeeds;
  §2 the SAME code again in the same hub is refused with `pricing.duplicate_code` — the stable code
     the screen translates and hangs on the «Code» field. Asserted by CODE, never by the prose
     (ADR-0055): the message is translated business text;
  §3 the SAME code in ANOTHER hub is created: uniqueness is `(hub_id, code)`, and one shop's
     tariff must never block another's (nor reveal that it exists);
  §4 the refusal wrote nothing: the first hub still holds exactly one list with that code.

Why against the runtime: the check is a `reads` block the host resolves under the tenant it
injects. A scratch Postgres that binds `:hub_id` by hand cannot prove the host scopes that read.

Usage: tests/duplicate_list_code.hub.test.py   (exit 0 = green)
  Needs a live runtime with `pricing` installed: `erplora test <dir> --against-hub`
  (`ERPLORA_HUB_BASE_URL`). Without one it FAILS — it never skips into green.
"""

import json
import os
import sys
import urllib.error
import urllib.request
import uuid

BASE = (os.environ.get("ERPLORA_HUB_BASE_URL") or "").rstrip("/")
BATTERY = "duplicate_list_code.hub"
DUPLICATE_CODE = "pricing.duplicate_code"

failures: list[str] = []


def request(method: str, path: str, hub_id: str | None = None, body=None):
    headers = {"content-type": "application/json"}
    if hub_id:
        headers["x-hub-id"] = hub_id
    req = urllib.request.Request(
        f"{BASE}{path}",
        data=None if body is None else json.dumps(body).encode(),
        headers=headers,
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as res:
            return res.status, json.loads(res.read().decode() or "null")
    except urllib.error.HTTPError as err:
        raw = err.read().decode()
        try:
            return err.code, json.loads(raw or "null")
        except json.JSONDecodeError:
            return err.code, {"raw": raw}


def create(hub_id: str, code: str):
    return request(
        "POST",
        "/api/command",
        hub_id,
        {
            "name": "pricing.price_lists.create",
            "payload": {
                "code": code,
                "name": f"Offers {code}",
                "currency": "EUR",
                "segment": None,
                "is_default": False,
                "tax_included": None,
                "valid_from": None,
                "valid_until": None,
            },
        },
    )


def lists_with_code(hub_id: str, code: str) -> int:
    status, body = request(
        "POST",
        "/api/query",
        hub_id,
        {"name": "pricing.price_lists.code_taken", "params": {"code": code}},
    )
    if status != 200:
        failures.append(f"code_taken query failed (HTTP {status}): {body}")
        return -1
    data = (body or {}).get("data", body)
    rows = data.get("rows", data) if isinstance(data, dict) else data
    return len(rows or [])


def check(label: str, got, want) -> None:
    if got != want:
        failures.append(f"{label} — expected [{want!r}], got [{got!r}]")
        print(f"  FAIL: {label} — expected [{want!r}], got [{got!r}]")
    else:
        print(f"  ok: {label} = {got!r}")


def error_code(body) -> str | None:
    return (
        ((body or {}).get("error") or {}).get("code")
        if isinstance(body, dict)
        else None
    )


def main() -> int:
    if not BASE:
        print(
            f"{BATTERY}: no runtime at the other end (ERPLORA_HUB_BASE_URL is empty)."
        )
        print(
            "Run it with `erplora test <dir> --against-hub`; without a hub this is a FAILURE."
        )
        return 1
    status, ctx = request("GET", "/api/hub/context")
    hub_a = (ctx or {}).get("hub_id") if status == 200 else None
    if not hub_a:
        print(
            f"{BATTERY}: GET /api/hub/context did not say the hub_id (HTTP {status}): {ctx}"
        )
        return 1
    hub_b = str(uuid.uuid4())
    # A fresh code per run: the hub may be long-lived and shared by earlier runs.
    code = f"OFF-{uuid.uuid4().hex[:8].upper()}"

    print(f"§1 the first list with code {code} is created in hub A")
    status, body = create(hub_a, code)
    check("§1 HTTP status", status, 200)

    print("§2 the same code again in hub A is refused with its domain code")
    status, body = create(hub_a, code)
    check("§2 HTTP status", status, 409)
    check("§2 error code", error_code(body), DUPLICATE_CODE)

    print("§3 the same code in hub B is allowed (uniqueness is per hub)")
    status, body = create(hub_b, code)
    check("§3 HTTP status", status, 200)
    check("§3 no error", error_code(body), None)

    print("§4 the refusal wrote nothing: hub A holds exactly one list with that code")
    check("§4 lists with the code in hub A", lists_with_code(hub_a, code), 1)
    check("§4 lists with the code in hub B", lists_with_code(hub_b, code), 1)

    print()
    if failures:
        print(f"✗ {BATTERY}: {len(failures)} failure(s):")
        for f in failures:
            print(f"  - {f}")
        return 1
    print(
        f"✓ {BATTERY}: a repeated list code is refused per hub, and another hub may reuse it"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
