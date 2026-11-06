"""World 4 data that isn't a plain table.

The Cape Vine takes every cape the game lets you deposit: the equipment whose `typeGen` is
"aCape", except Lava Wings (`EquipmentCape1`) and items that can't be crafted (`lvReqToCraft`
69420), as the game counts them when it draws the vine (`MenuType2` 110). Each equipment item is
defined as `...displayName="Name",...typeGen="aCape",...lvReqToCraft="55",... N.addNewEquip("ID",e)`.
Kept here: [ID, name] for each such cape, in definition order.
"""

import re

EQUIP = re.compile(r'addNewEquip\("([^"]+)",e\)')


def capes(js: str) -> list[list[str]]:
    out = []
    last = 0
    for m in EQUIP.finditer(js):
        body = js[last:m.start()]
        last = m.end()
        start = body.rfind('displayName="')
        if start < 0:
            continue
        body = body[start:]
        name = re.match(r'displayName="([^"]*)"', body).group(1)
        kind = re.search(r'typeGen="([^"]*)"', body)
        craft = re.search(r'lvReqToCraft="?([^",]*)"?', body)
        if not kind or kind.group(1) != "aCape" or m.group(1) == "EquipmentCape1":
            continue
        if craft and craft.group(1) == "69420":
            continue
        # "|" is a line break in the game's text.
        out.append([m.group(1), name.replace("|", "_")])
    return out


def extract(js: str, tables: dict) -> dict:
    return {"capes": capes(js)}
