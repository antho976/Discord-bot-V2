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


def split(m: re.Match | None, sep: str) -> list[str]:
    return [x for x in m.group(1).split(sep) if x] if m else []


def extract(js: str, tables: dict) -> dict:
    return {
        "clam_upgrades": split(CLAMS.search(js), " "),
        "dancing_coral_paid": split(CORALS.search(js), ";"),
        "dancing_coral_from": [x.strip("()") for x in split(SOURCES.search(js), ";")],
    }
