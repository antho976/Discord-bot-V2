"""World 7 data that isn't a plain table.

The Clamworks upgrade names are only in the code that draws the Clamworks page, as one text
split on spaces: `"PEARL_VALUE CLAM_COMRADES ... TRUE_SCURGE".split(" ")[slot + 9 * page]`.
Slots 0-8 are the pearl upgrades (levels in `OptLacc[455 + slot]`), 9-17 the Musselini
upgrades (levels in `Menagerie[5][slot]`).
"""

import re

CLAMS = re.compile(r'"(PEARL_VALUE [A-Z0-9_ \-]+)"\.split\(" "\)')


def extract(js: str, tables: dict) -> dict:
    m = CLAMS.search(js)
    return {"clam_upgrades": m.group(1).split(" ") if m else []}
