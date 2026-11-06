"""World 3 data that isn't a plain table.

- Monster faces: the Death Note draws each monster as `"Mface"+MonsterFace+".png"`, where
  `MonsterFace` is set in `addNewMonster("mushG",{Name:"...",AFKtype:"...",MonsterFace:1,...})`.
  Only the Death Note's monsters (`DeathNoteMobs`) and minibosses (`NinjaInfo[30]`) are kept.
- Armor sets: `EquipmentSets` is a map, `e.h.COPPER_SET=[["EquipmentHats17",...],...]`, not a
  plain list, so the set names come from its keys in order, with its armor pieces (the first
  list, written either as quoted items or as `"A B C".split(" ")`); the hat is the picture.
  The save lists finished sets by name in `OptLacc[379]`.
- Refinery salt unlocks: the unlock button pays `_GenINFO[69][1 + k]` of salt "Refinery{k}" to
  raise `Refinery[0][0]` from k. That list is built in the refinery actor's init as the 70th
  `_GenINFO.push`, from the `_DummyList.push(...)` numbers just before it.
"""

import re

MONSTER = re.compile(r'addNewMonster\("([^"]+)",\{Name:"[^"]*",AFKtype:"[^"]*",MonsterFace:(-?\d+)')
SET = re.compile(r'e\.h\.([A-Z0-9_]+_SET)=\[(\[[^\]]*\]|"[^"]*"\.split\(" "\))')


UNLOCK = 'a.engine.getGameAttribute("Refinery")[0][0]=Math.round(c.asNumber(a.engine.getGameAttribute("Refinery")[0][0])+1)'


def salt_unlocks(js: str) -> list:
    at = js.find(UNLOCK)
    if at < 0:
        return []
    start = js.rfind("__class__:", 0, at)
    end = js.find("__class__:", at)
    cls = js[start:end]
    init = cls.find("this._GenINFO=[]")
    if init < 0:
        return []
    pushes = list(re.finditer(r"this\._GenINFO\.push\(", cls[init:]))
    if len(pushes) < 70:
        return []
    seg = cls[init + pushes[68].start():init + pushes[69].start()]
    return [float(x) for x in re.findall(r"_DummyList\.push\(([0-9.e]+)\)", seg)]


def extract(js: str, tables: dict) -> dict:
    faces = {m.group(1): int(m.group(2)) for m in MONSTER.finditer(js)}
    mobs = [m for world in tables.get("DeathNoteMobs", []) for m in world]
    ninja = tables.get("NinjaInfo", [])
    minis = ninja[30] if len(ninja) > 30 else []
    start = js.find(".EquipmentSets=function(){")
    end = js.find("return e}", start) if start >= 0 else -1
    sets = []
    for m in SET.finditer(js, start, end) if end > 0 else []:
        first = m.group(2)
        pieces = re.findall(r'"([^"]+)"', first)
        if first.startswith('"'):
            pieces = pieces[0].split(" ")
        if pieces:
            sets.append([m.group(1), pieces[0], pieces])
    return {
        "faces": {m: faces[m] for m in mobs + minis if m in faces},
        "sets": sets,
        "salt_unlocks": salt_unlocks(js),
    }
