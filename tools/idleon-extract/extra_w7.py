"""World 7 data that isn't a plain table.

The Clamworks upgrade names are only in the code that draws the Clamworks page, as one text
split on spaces: `"PEARL_VALUE CLAM_COMRADES ... TRUE_SCURGE".split(" ")[slot + 9 * page]`.
Slots 0-8 are the pearl upgrades (levels in `OptLacc[455 + slot]`), 9-17 the Musselini
upgrades (levels in `Menagerie[5][slot]`).

The Dancing Coral page names what each bonus is paid with and where that comes from, also as
texts split on ";": `"Reef_Coral;Vibrant_Coral;..."` and `"(Generated_Daily_in_Town);..."`.
"""

import re

CLAMS = re.compile(r'"(PEARL_VALUE [A-Z0-9_ \-]+)"\.split\(" "\)')


CORALS = re.compile(r'"(Reef_Coral;[A-Za-z_;]+)"')
SOURCES = re.compile(r'"(\(Generated_Daily_in_Town\);[A-Za-z_;()\-]+)"')


QUEST = re.compile(r'Rewards:\[([^\]]*)\],QuestName:"([^"]+)"')
DROP = re.compile(r'\["((?:Trophy|EquipmentNametag)\d+b?)","([0-9.]+)","\d+","N/A"\]')
TABLE = re.compile(r'e\.h\.(\w+)=\[')
BUNDLE = re.compile(r'"(bun_[a-z0-9]+)"==[^;]{0,200}?GiveItem\("((?:Trophy|EquipmentNametag)\d+b?)"')
OPENED = re.compile(r'"(Quest\d+)"==""\+h\.string\(a\.engine\.getGameAttribute\("InventoryOrder"\)\[0\|this\._ItemDragID\]\)\)[^;]{0,400}?DummyText2="((?:Trophy|EquipmentNametag)\d+b?)"')
GIVEN = re.compile(r'(?:GiveItem|DropSomething)\("((?:Trophy|EquipmentNametag)\d+b?)"')
GALLERY = re.compile(r'"((?:Trophy|EquipmentNametag)\d+b?)"')


def gallery_sources(js: str) -> dict:
    out: dict[str, list] = {}

    def add(item: str, entry: list):
        if entry not in out.setdefault(item, []):
            out[item].append(entry)

    for m in QUEST.finditer(js):
        for item in GALLERY.findall(m.group(1)):
            add(item, ["quest", m.group(2)])
    for m in DROP.finditer(js):
        start = js.rfind("e.h.", 0, m.start())
        t = TABLE.match(js, start)
        add(m.group(1), ["drop", t.group(1) if t else ""])
    for m in BUNDLE.finditer(js):
        add(m.group(2), ["pack", m.group(1)])
    for m in OPENED.finditer(js):
        add(m.group(2), ["open", m.group(1)])
    for m in GIVEN.finditer(js):
        before = js[max(0, m.start() - 4000):m.start()]
        if "DartsPopup" in before:
            add(m.group(1), ["darts", ""])
        elif '"ROLL_DA_DICE"' in before[-2500:]:
            add(m.group(1), ["dice", ""])
    return out


def split(m: re.Match | None, sep: str) -> list[str]:
    return [x for x in m.group(1).split(sep) if x] if m else []


def extract(js: str, tables: dict) -> dict:
    return {
        "clam_upgrades": split(CLAMS.search(js), " "),
        "dancing_coral_paid": split(CORALS.search(js), ";"),
        "dancing_coral_from": [x.strip("()") for x in split(SOURCES.search(js), ";")],
        "gallery_sources": gallery_sources(js),
    }
