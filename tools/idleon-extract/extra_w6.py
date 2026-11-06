"""World 6 data that isn't a plain table.

Sneaking's charms and gemstones live in `NjEQ`, a map built as `e.h.NjTrP0="3 0 Sparkle_Log 20
...".split(" ")`. Kept here: the names, in index order, of the Pristine Charms (`NjTrP{i}`, owned
when `Ninja[107][i]` is 1) and the Gemstones (`NjGem{i}`, level in `OptLacc[233 + i]`).
"""

import re


def _names(js: str, prefix: str) -> list[str]:
    found = {}
    for m in re.finditer(r'e\.h\.' + prefix + r'(\d+)="([^"]*)"\.split\(" "\)', js):
        parts = m.group(2).split(" ")
        if len(parts) > 2:
            found.setdefault(int(m.group(1)), parts[2])
    return [found[i] for i in range(len(found)) if i in found]


def extract(js: str, tables: dict) -> dict:
    return {"pristine": _names(js, "NjTrP"), "gems": _names(js, "NjGem")}
