"""Account-wide data that isn't a plain table.

- Stackable items: every item's `typeGen` is set just before it is registered
  (`t.h.typeGen="bBar", ... N.addNewItem("CopperBar",e)`). Types starting with `a` are
  equipment and other one-per-slot things; `b`, `c` and `d` stack (materials, food, quest
  items, currencies). Cards (`dCard`) never sit in storage, so they're left out.
- Gem shop restocks: the game sets `GemItemsPurchased[i]=0` for things that restock every day
  or week, and in the one-time update fixes (`DoOnceREAL`) for the limited stock it rotates.
  A count that the game resets outside an update fix, or in two or more update fixes, is
  stock, not a permanent purchase.
"""

import re

ITEM = re.compile(r'\.addNew(?:Quest|Equip|Consumable|Item)\("([^"]+)",e\)')
TYPE = re.compile(r'typeGen="([^"]*)"')
RESET = re.compile(r'GemItemsPurchased"\)\[(\d+)\]=0[,;)]')


def stackable(js: str) -> list[str]:
    out, last = [], 0
    for m in ITEM.finditer(js):
        types = TYPE.findall(js, last, m.start())
        last = m.end()
        if types and types[-1][:1] in "bcd" and types[-1] != "dCard":
            out.append(m.group(1))
    return sorted(set(out))


def restocks(js: str) -> list[int]:
    plain, fixes = set(), {}
    for m in RESET.finditer(js):
        # Walk back over the run of resets this one belongs to.
        start = m.start()
        while True:
            k = js.rfind('GemItemsPurchased")[', 0, start)
            if k < 0 or start - k > 80:
                break
            start = k
        i = int(m.group(1))
        fix = js.rfind("DoOnceREAL", start - 1500, start)
        if fix < 0:
            plain.add(i)
        else:
            fixes.setdefault(i, set()).add(fix)
    return sorted(plain | {i for i, f in fixes.items() if len(f) >= 2})


def extract(js: str, tables: dict) -> dict:
    return {"stackable": stackable(js), "gem_restocks": restocks(js)}
