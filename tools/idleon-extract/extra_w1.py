"""World 1 data that isn't a plain table.

- Megafeathers: Orion's ten Megafeather names are one comma-separated text pushed right before
  their descriptions (`_GenINFO.push("Maple,...,Fractal"),_GenINFO.push("Multiplies_all_Feather...`).
  The owl reads `OptLacc[262] > i` for Megafeather i; the tenth repeats without end.
- Hoops and Darts: each shop draws its upgrade text from an inline list whose first entry is
  "Select_an_upgrade,...", then fills in `OptLacc[first + i]` (419 for Hoops, 435 for Darts).
- Forge: the upgrade names are `_FurnaceUpgradeText.push("NAME|LINE|LINE")`, in `ForgeLV` order.
- Anvil: the coin and monster-part points stop at a number written in the buy button's check,
  `600>N(AnvilPAstats[1])` and `700>N(AnvilPAstats[2])`.
"""

import json
import re

FEATHERS = re.compile(r'_GenINFO\.push\("([A-Za-z]+(?:,[A-Za-z]+)+)"\),this\._GenINFO\.push\("Multiplies_all_Feather')
SHOP = re.compile(r'\[("Select_an_upgrade,_these_bonuses[^\]]*)\]\[Math\.round\(')
SHOP_SLOT = re.compile(r'OptionsListAccount"\)\[Math\.round\((\d+)\+')
FORGE = re.compile(r'_FurnaceUpgradeText\.push\("([^"|]+)\|')
ANVIL = r'if\((\d+)>c\.asNumber\(a\.engine\.getGameAttribute\("AnvilPAstats"\)\[{}\]\)&&'


def shops(js: str) -> dict:
    out = {}
    for m in SHOP.finditer(js):
        slot = SHOP_SLOT.search(js, m.end(), m.end() + 400)
        if not slot:
            continue
        first = int(slot.group(1))
        name = {419: "hoops", 435: "darts"}.get(first)
        if name and name not in out:
            out[name] = {"first": first, "upgrades": json.loads("[" + m.group(1) + "]")[1:]}
    return out


def extract(js: str, tables: dict) -> dict:
    feathers = FEATHERS.search(js)
    forge = []
    for m in FORGE.finditer(js):
        if m.group(1) in forge:
            break
        forge.append(m.group(1))
    anvil = [re.search(ANVIL.format(i), js) for i in (1, 2)]
    return {
        "megafeathers": feathers.group(1).split(",") if feathers else [],
        **shops(js),
        "forge_upgrades": forge,
        "anvil_points": [int(m.group(1)) if m else 0 for m in anvil],
    }
