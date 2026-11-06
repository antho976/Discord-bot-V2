"""World 3 data that isn't a plain table.

- Monster faces: the Death Note draws each monster as `"Mface"+MonsterFace+".png"`, where
  `MonsterFace` is set in `addNewMonster("mushG",{Name:"...",AFKtype:"...",MonsterFace:1,...})`.
  Only the Death Note's monsters (`DeathNoteMobs`) and minibosses (`NinjaInfo[30]`) are kept.
- Armor sets: `EquipmentSets` is a map, `e.h.COPPER_SET=[["EquipmentHats17",...],...]`, not a
  plain list, so the set names come from its keys in order, with the first armor piece (the
  hat) for the picture. The save lists finished sets by name in `OptLacc[379]`.
"""

import re

MONSTER = re.compile(r'addNewMonster\("([^"]+)",\{Name:"[^"]*",AFKtype:"[^"]*",MonsterFace:(-?\d+)')
SET = re.compile(r'e\.h\.([A-Z0-9_]+_SET)=\[\[?"(Equipment[A-Za-z]+\d+)')


def extract(js: str, tables: dict) -> dict:
    faces = {m.group(1): int(m.group(2)) for m in MONSTER.finditer(js)}
    mobs = [m for world in tables.get("DeathNoteMobs", []) for m in world]
    ninja = tables.get("NinjaInfo", [])
    minis = ninja[30] if len(ninja) > 30 else []
    start = js.find(".EquipmentSets=function(){")
    end = js.find("return e}", start) if start >= 0 else -1
    sets = [[m.group(1), m.group(2)] for m in SET.finditer(js, start, end)] if end > 0 else []
    return {
        "faces": {m: faces[m] for m in mobs + minis if m in faces},
        "sets": sets,
    }
