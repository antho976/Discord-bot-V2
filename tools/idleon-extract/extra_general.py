"""Account-wide data that isn't a plain table.

- Stackable items: every item's `typeGen` is set just before it is registered
  (`t.h.typeGen="bBar", ... N.addNewItem("CopperBar",e)`). Types starting with `a` are
  equipment and other one-per-slot things; `b`, `c` and `d` stack (materials, food, quest
  items, currencies). Cards (`dCard`) never sit in storage, so they're left out.
- Gem shop restocks: the game sets `GemItemsPurchased[i]=0` for things that restock every day
  or week, and in the one-time update fixes (`DoOnceREAL`) for the limited stock it rotates.
  A count that the game resets outside an update fix, or in two or more update fixes, is
  stock, not a permanent purchase.
- Drops: `X.MonsterDrops=function(){var e=new l;return e.h.mushG=[["item","chance","qty",
  "quest"],...],...,e}`, and the shared `X.DropTables` (`DropTable15`, `SuperDropTable3`) in the
  same shape. A key is a monster ID (registered with `addNewMonster`) or a shared table that
  other lists name as an item; shared
  tables are followed (chances multiplied, at most 6 deep). Kept as {item ID: [monster IDs]},
  most likely first; coins are left out.
- Quest rewards: NPC dialogue nodes carry `Rewards:["item",qty,...],...,QuestName:"Krunk2"`; kept
  as {item ID: [quest names]}. The NPC is the quest name without its number.
- Other sources, kept as {item ID: source}: "arcade" for the Arcade's prizes
  (`givReward("StampC4",1,"arcade")`), and "fishing_island" for what the Fishing Island shop
  (scene zFishingD) hands out for `OptLacc[161]` (`DropSomething("StampB47",...)` after the
  `N(OptLacc[161])>=N(this._GenINFO[2][t])` price check).
"""

import json
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


DROPS = re.compile(r'\.(?:MonsterDrops|DropTables)=function\(\)\{var e=new l;return (.*?),e\}')
DROP = re.compile(r'e\.h\.(\w+)=(\[\[.*?\]\])(?=,e\.h\.|$)')
MONSTER = re.compile(r'addNewMonster\("([^"]+)"')


def drops(js: str) -> dict:
    lists = {}
    for body in DROPS.finditer(js):
        for m in DROP.finditer(body.group(1)):
            try:
                lists[m.group(1)] = json.loads(m.group(2))
            except ValueError:
                continue
    monsters = set(MONSTER.findall(js))
    best: dict = {}

    def walk(mob, key, chance, depth):
        for row in lists.get(key, []):
            if not isinstance(row, list) or len(row) < 2:
                continue
            item = str(row[0])
            try:
                c = chance * float(row[1])
            except ValueError:
                continue
            if item in lists and item not in monsters:
                if depth < 6:
                    walk(mob, item, c, depth + 1)
            elif item != "COIN":
                seen = best.setdefault(item, {})
                seen[mob] = max(seen.get(mob, 0.0), c)

    for key in lists:
        if key in monsters:
            walk(key, key, 1.0, 0)
    return {item: [m for m, _ in sorted(mobs.items(), key=lambda kv: -kv[1])] for item, mobs in sorted(best.items())}


QUEST = re.compile(r'Rewards:\[([^\]]*)\][^}]*?QuestName:"(\w+)"')


def quests(js: str) -> dict:
    out: dict = {}
    for m in QUEST.finditer(js):
        for item in re.findall(r'"(\w+)"', m.group(1)):
            names = out.setdefault(item, [])
            if m.group(2) not in names:
                names.append(m.group(2))
    return out


REWARD = re.compile(r'givReward\("(\w+)",[^,]+,"(\w+)"\)')
ISLAND = re.compile(r'getGameAttribute\("OptionsListAccount"\)\[161\]\)>=c\.asNumber\(this\._GenINFO\[2\]\[t\]\)(.{0,900})')


def sources(js: str) -> dict:
    out = {item: where for item, where in REWARD.findall(js)}
    island = ISLAND.search(js)
    for item in re.findall(r'DropSomething\("(\w+)"', island.group(1)) if island else []:
        out.setdefault(item, "fishing_island")
    return out


def extract(js: str, tables: dict) -> dict:
    return {"stackable": stackable(js), "gem_restocks": restocks(js), "drops": drops(js), "quests": quests(js), "sources": sources(js)}
