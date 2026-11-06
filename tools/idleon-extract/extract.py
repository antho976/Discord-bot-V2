#!/usr/bin/env python3
"""Pull Idleon's game data and pictures out of the Steam install, for crates/feat-idleon.

The game is an Electron app. Its code and data are one script, N.js, inside resources/app.asar,
and its pictures are one Lime asset pack, lib/default.pak, indexed by a manifest inside N.js.

Data: the game keeps its tables as functions that return a literal, `X.StampDescriptions =
function(){return [...]}`. This reads those literals with a small parser; it never runs the
game's code. Each table is written as JSON exactly as the game has it (strings stay strings).

Pictures: the manifest gives each picture's place in the pack and its name, which is the game's
own ID ("StampA1", "FarmCrop12"); the English names come from the item tables. Only the pictures
the crate asks for (see PICTURES) are written.

Run after a game update:
    python3 -I tools/idleon-extract/extract.py
    python3 -I tools/idleon-extract/extract.py --game "/path/to/Legends of Idleon" --all-tables /tmp/tables.json
"""

from __future__ import annotations

import argparse
import gzip
import json
import re
import struct
import sys
import zlib
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import unquote

STEAM = Path.home() / ".local/share/Steam/steamapps/common/Legends of Idleon"
CRATE = Path(__file__).resolve().parents[2] / "crates/feat-idleon"

HERE = Path(__file__).resolve().parent


def needs() -> tuple[list[str], list[str], list[tuple[str, list[int], int]]]:
    """What the checks need, one file per area in needs/: tables, picture patterns, and pictures
    named by a table cell. See needs/w1.txt for the format."""
    tables, pictures, cells = [], [], []
    for f in sorted((HERE / "needs").glob("*.txt")):
        for n, line in enumerate(f.read_text().splitlines(), 1):
            line = line.split(" #")[0].strip()
            if not line or line.startswith("#"):
                continue
            kind, _, rest = line.partition(" ")
            if kind == "table":
                tables.append(rest.strip())
            elif kind == "picture":
                pictures.append(rest.strip())
            elif kind == "cell":
                t, path, col = rest.split()
                cells.append((t, [] if path == "-" else [int(x) for x in path.split(",")], int(col)))
            else:
                sys.exit(f"{f.name}:{n}: unknown entry {kind!r}")
    return list(dict.fromkeys(tables)), list(dict.fromkeys(pictures)), cells


def extras(js: str, found: dict) -> dict:
    """Data that isn't a plain table: each extra_<area>.py next to this file has
    `extract(js, tables) -> JSON-able value`, kept under game.json "extra"/<area>."""
    import importlib.util
    out = {}
    for f in sorted(HERE.glob("extra_*.py")):
        spec = importlib.util.spec_from_file_location(f.stem, f)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        out[f.stem.removeprefix("extra_")] = mod.extract(js, found)
    return out


# --- app.asar -----------------------------------------------------------------------------------

class Asar:
    def __init__(self, path: Path):
        self.f = path.open("rb")
        self.f.read(4)
        header_size = struct.unpack("<I", self.f.read(4))[0]
        self.f.read(4)
        json_len = struct.unpack("<I", self.f.read(4))[0]
        self.header = json.loads(self.f.read(json_len))
        self.base = 8 + header_size

    def read(self, name: str) -> bytes:
        node = self.header
        for part in name.strip("/").split("/"):
            node = node["files"][part]
        self.f.seek(self.base + int(node["offset"]))
        return self.f.read(int(node["size"]))


# --- JS literals --------------------------------------------------------------------------------

class Unsupported(Exception):
    pass


class Literal:
    """Reads one JS expression made of arrays, strings, numbers, `"a b".split(" ")`, `!0`, `!1`
    and `null`. Anything else (a variable, a call) raises Unsupported."""

    NUM = re.compile(r"-?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?")

    def __init__(self, s: str, i: int):
        self.s, self.i = s, i

    def ws(self):
        while self.s[self.i] in " \n\t\r":
            self.i += 1

    def value(self):
        self.ws()
        c = self.s[self.i]
        if c == "[":
            v = self.array()
        elif c in "\"'":
            v = self.string()
        elif c == "!" and self.s[self.i + 1] in "01":
            v = self.s[self.i + 1] == "0"
            self.i += 2
        elif self.s.startswith("null", self.i):
            v = None
            self.i += 4
        else:
            m = self.NUM.match(self.s, self.i)
            if not m:
                raise Unsupported(self.s[self.i:self.i + 40])
            self.i = m.end()
            t = m.group()
            v = float(t) if any(ch in t for ch in ".eE") else int(t)
        return self.methods(v)

    def methods(self, v):
        while True:
            if self.s.startswith(".split(", self.i) and isinstance(v, str):
                self.i += len(".split(")
                sep = self.string()
                if self.s[self.i] != ")":
                    raise Unsupported("split with a limit")
                self.i += 1
                v = list(v) if sep == "" else v.split(sep)
            else:
                return v

    def array(self):
        self.i += 1
        out = []
        self.ws()
        if self.s[self.i] == "]":
            self.i += 1
            return out
        while True:
            out.append(self.value())
            self.ws()
            c = self.s[self.i]
            self.i += 1
            if c == "]":
                return out
            if c != ",":
                raise Unsupported(f"{c!r} in an array")

    def string(self):
        q = self.s[self.i]
        self.i += 1
        out = []
        while True:
            c = self.s[self.i]
            if c == q:
                self.i += 1
                return "".join(out)
            if c == "\\":
                n = self.s[self.i + 1]
                if n == "u":
                    out.append(chr(int(self.s[self.i + 2:self.i + 6], 16)))
                    self.i += 6
                    continue
                out.append({"n": "\n", "t": "\t", "r": "\r"}.get(n, n))
                self.i += 2
                continue
            out.append(c)
            self.i += 1


TABLE_DEF = re.compile(r"\.([A-Za-z_][A-Za-z0-9_]*)=function\(\)\{return")


def tables(js: str) -> tuple[dict, dict]:
    found, failed = {}, {}
    for m in TABLE_DEF.finditer(js):
        name = m.group(1)
        try:
            p = Literal(js, m.end())
            v = p.value()
            p.ws()
            if js[p.i] != "}":
                raise Unsupported("more after the literal")
        except (Unsupported, IndexError, ValueError) as e:
            failed[name] = str(e)[:80]
            continue
        if isinstance(v, list) and name not in found:
            found[name] = v
    return found, failed


# --- items: ID -> English name ------------------------------------------------------------------

ITEM = re.compile(r'\.addNewQuest\("([^"]+)",e\)')
DISPLAY = re.compile(r'displayName="([^"]*)"')


def item_names(js: str) -> dict[str, str]:
    """`t.h.displayName="Heart_Stamp", ... e.h.common=t, P.addNewQuest("StampA1",e)`: each item's
    display name is the last one set before it is registered."""
    out = {}
    last = 0
    for m in ITEM.finditer(js):
        names = DISPLAY.findall(js, last, m.start())
        if names:
            out[m.group(1)] = names[-1].replace("_", " ")
        last = m.end()
    return out


MONSTER = re.compile(r'addNewMonster\("([^"]+)",\{Name:"([^"]*)"')


def monster_names(js: str) -> dict[str, str]:
    return {m.group(1): m.group(2).replace("_", " ") for m in MONSTER.finditer(js)}


# --- default.pak --------------------------------------------------------------------------------

class Haxe:
    """Reads Haxe's serialization format, as far as Lime's asset manifest uses it."""

    def __init__(self, s: str):
        self.s, self.i, self.cache = s, 0, []

    def int_(self):
        m = re.compile(r"-?\d+").match(self.s, self.i)
        self.i = m.end()
        return int(m.group())

    def value(self):
        c = self.s[self.i]
        self.i += 1
        if c == "o":
            obj = {}
            while self.s[self.i] != "g":
                k = self.value()
                obj[k] = self.value()
            self.i += 1
            return obj
        if c == "a":
            arr = []
            while self.s[self.i] != "h":
                arr.append(self.value())
            self.i += 1
            return arr
        if c == "y":
            n = self.int_()
            self.i += 1  # ':'
            v = unquote(self.s[self.i:self.i + n])
            self.i += n
            self.cache.append(v)
            return v
        if c == "R":
            return self.cache[self.int_()]
        if c == "i":
            return self.int_()
        if c == "z":
            return 0
        if c == "t":
            return True
        if c == "f":
            return False
        if c == "n":
            return None
        raise Unsupported(f"haxe {c!r} at {self.i}")


def manifest(js: str) -> list[dict]:
    end = js.find('lib/default.pak","gzip"')
    start = js.rfind('"assets":"', 0, end)
    raw = js[start + len('"assets":"'):js.find('"', start + len('"assets":"'))]
    return Haxe(raw).value()


def picture(pak: bytes, entry: dict) -> bytes:
    data = pak[entry["position"]:entry["position"] + entry["length"]]
    return zlib.decompress(data, 31) if data[:2] == b"\x1f\x8b" else data


# --- main ---------------------------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--game", type=Path, default=STEAM, help="the game's folder (default: Steam's)")
    ap.add_argument("--out", type=Path, default=CRATE, help="the crate to write into")
    ap.add_argument("--all-tables", type=Path, help="also write every table the game has to this file")
    ap.add_argument("--all-pictures", type=Path, help="also write every picture to this folder, under its game ID")
    args = ap.parse_args()

    asar = Asar(args.game / "resources/app.asar")
    js = asar.read("distBuild/static/game/N.js").decode("utf-8", "replace")

    found, failed = tables(js)
    TABLES, PICTURES, PICTURE_CELLS = needs()
    missing = [t for t in TABLES if t not in found]
    if missing:
        sys.exit(f"tables not found or unreadable: {', '.join(missing)}")
    version = re.search(r'"(\d+\.\d+(?:\.\d+)?)"\s*,\s*[A-Za-z_.]*\s*=\s*"Patch', js)
    names = item_names(js)

    built = datetime.fromtimestamp((args.game / "resources/app.asar").stat().st_mtime, timezone.utc)
    data = {
        "updated": built.strftime("%Y-%m-%d"),
        "tables": {t: found[t] for t in TABLES},
        "items": names,
        "monsters": monster_names(js),
        "extra": extras(js, found),
    }
    out_data = args.out / "data/game.json"
    out_data.parent.mkdir(parents=True, exist_ok=True)
    out_data.write_text(json.dumps(data, separators=(",", ":"), ensure_ascii=False) + "\n")
    print(f"{out_data}: {len(TABLES)} tables, {len(names)} item and {len(data['monsters'])} monster names, {out_data.stat().st_size // 1024} KiB")

    pak = gzip_safe(asar.read("distBuild/static/game/lib/default.pak"))
    images = [e for e in manifest(js) if e.get("type") == "IMAGE"]
    cells = set()
    for table, path, col in PICTURE_CELLS:
        rows = found[table]
        for i in path:
            rows = rows[i]
        cells |= {r[col] for r in rows if len(r) > col}
    write_pictures(args.out, pak, images, cells, PICTURES)

    if args.all_tables:
        args.all_tables.write_text(json.dumps(found, ensure_ascii=False))
        print(f"{args.all_tables}: all {len(found)} tables")

    if args.all_pictures:
        args.all_pictures.mkdir(parents=True, exist_ok=True)
        for e in images:
            (args.all_pictures / Path(e["id"]).name).write_bytes(picture(pak, e))
        print(f"{args.all_pictures}: {len(images)} pictures")


def write_pictures(crate: Path, pak: bytes, images: list[dict], names: set[str], patterns: list[str]):
    """Copy the pictures PICTURES names into crate/pictures and list them in src/pictures.rs."""
    wanted = [re.compile(p) for p in patterns]
    chosen = {}
    for e in images:
        path = Path(e["id"])
        if path.suffix != ".png" or len(path.parts) < 3 or path.parts[1] != "data":
            continue
        if path.stem in names or any(w.fullmatch(path.stem) for w in wanted):
            chosen[path.stem] = picture(pak, e)
    folder = crate / "pictures"
    folder.mkdir(exist_ok=True)
    for old in folder.glob("*.png"):
        if old.stem not in chosen:
            old.unlink()
    for name, png in chosen.items():
        (folder / f"{name}.png").write_bytes(png)
    lines = [
        "//! Written by tools/idleon-extract/extract.py: the game's pictures the checks show, by game ID.",
        "",
        "/// Sorted by ID.",
        "static PICTURES: &[(&str, &[u8])] = &[",
    ]
    for name in sorted(chosen):
        lines.append(f'    ("{name}", include_bytes!("../pictures/{name}.png")),')
    lines += [
        "];",
        "",
        "pub fn get(id: &str) -> Option<&'static [u8]> {",
        "    PICTURES.binary_search_by(|(k, _)| (*k).cmp(id)).ok().map(|i| PICTURES[i].1)",
        "}",
        "",
        "pub fn has(id: &str) -> bool {",
        "    get(id).is_some()",
        "}",
        "",
    ]
    (crate / "src/pictures.rs").write_text("\n".join(lines))
    size = sum(len(p) for p in chosen.values())
    print(f"{folder}: {len(chosen)} pictures, {size // 1024} KiB")


def gzip_safe(pak: bytes) -> bytes:
    if not pak.startswith(b"lime-asset-pack"):
        raise SystemExit("default.pak isn't a Lime asset pack any more")
    return pak


if __name__ == "__main__":
    main()
