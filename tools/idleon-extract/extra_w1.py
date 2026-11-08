"""World 1 data that isn't a plain table.

- Megafeathers: Orion's ten Megafeather names are one comma-separated text pushed right before
  their descriptions (`_GenINFO.push("Maple,...,Fractal"),_GenINFO.push("Multiplies_all_Feather...`).
  The owl reads `OptLacc[262] > i` for Megafeather i; the tenth repeats without end.
- Forge: the upgrade names are `_FurnaceUpgradeText.push("NAME|LINE|LINE")`, in `ForgeLV` order.
- Anvil: the coin and monster-part points stop at a number written in the buy button's check,
  `600>N(AnvilPAstats[1])` and `700>N(AnvilPAstats[2])`. The monster part a point costs is
  `Costs2TypeAnvilPA`: `5>N(AnvilPAstats[2])?"Grasslands1":15>...?"Grasslands2":...:"GalaxyC4"`,
  kept as [[below, item], ...] with the last item for everything past.
"""

import re

FEATHERS = re.compile(r'_GenINFO\.push\("([A-Za-z]+(?:,[A-Za-z]+)+)"\),this\._GenINFO\.push\("Multiplies_all_Feather')
FORGE = re.compile(r'_FurnaceUpgradeText\.push\("([^"|]+)\|')
PARTS = re.compile(r'"Costs2TypeAnvilPA"==e\)return (.*?);')
PART = re.compile(r'(\d+)>c\.asNumber\(a\.engine\.getGameAttribute\("AnvilPAstats"\)\[2\]\)\?"(\w+)"')
LAST = re.compile(r':"(\w+)"$')
ANVIL = r'if\((\d+)>c\.asNumber\(a\.engine\.getGameAttribute\("AnvilPAstats"\)\[{}\]\)&&'


def extract(js: str, tables: dict) -> dict:
    feathers = FEATHERS.search(js)
    forge = []
    for m in FORGE.finditer(js):
        if m.group(1) in forge:
            break
        forge.append(m.group(1))
    anvil = [re.search(ANVIL.format(i), js) for i in (1, 2)]
    parts = PARTS.search(js)
    ladder = [[int(n), item] for n, item in PART.findall(parts.group(1))] if parts else []
    last = LAST.search(parts.group(1)) if parts else None
    if last:
        ladder.append([None, last.group(1)])
    return {
        "anvil_parts": ladder,
        "megafeathers": feathers.group(1).split(",") if feathers else [],
        "forge_upgrades": forge,
        "anvil_points": [int(m.group(1)) if m else 0 for m in anvil],
    }
