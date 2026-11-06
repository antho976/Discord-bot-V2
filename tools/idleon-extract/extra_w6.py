"""World 6 data that isn't a plain table.

Sneaking's charms and gemstones live in `NjEQ`, a map built as `e.h.NjTrP0="3 0 Sparkle_Log 20
...".split(" ")`. Kept here:
- `pristine`: Pristine Charm names (`NjTrP{i}`, owned when `Ninja[107][i]` is 1), in index order.
- `gems`: Gemstone names (`NjGem{i}`, level in `OptLacc[233 + i]`), in index order.
- `gem_values`: each gemstone's [base, scale] (cols 3 and 5): N.js `GemstoneBonus` is
  base + scale × lv / (lv + 1000).
- `gold`: the Gold_ charms by ID, [kind, bonus type, base, cap] (cols 0, 1, 3, 5), for N.js
  `ItemStat` and `NinjaBonus` (type 11 is Gold Coupon, the Ninja Knowledge discount).
"""

import re


def _entries(js: str, prefix: str) -> dict[str, list[str]]:
    out = {}
    for m in re.finditer(r'e\.h\.(' + prefix + r'\d+)="([^"]*)"\.split\(" "\)', js):
        out.setdefault(m.group(1), m.group(2).split(" "))
    return out


def _ordered(entries: dict[str, list[str]], prefix: str) -> list[list[str]]:
    rows = []
    while f"{prefix}{len(rows)}" in entries:
        rows.append(entries[f"{prefix}{len(rows)}"])
    return rows


def _num(s: str) -> float:
    try:
        return float(s)
    except ValueError:
        return 0.0


def extract(js: str, tables: dict) -> dict:
    pristine = _ordered(_entries(js, "NjTrP"), "NjTrP")
    gems = _ordered(_entries(js, "NjGem"), "NjGem")
    gold = {
        k: [_num(v[0]), _num(v[1]), _num(v[3]), _num(v[5])]
        for k, v in _entries(js, "NjTr").items()
        if len(v) > 5 and "Gold_" in v[2]
    }
    return {
        "pristine": [p[2] for p in pristine if len(p) > 2],
        "gems": [g[2] for g in gems if len(g) > 2],
        "gem_values": [[_num(g[3]), _num(g[5])] for g in gems if len(g) > 5],
        "gold": gold,
    }
