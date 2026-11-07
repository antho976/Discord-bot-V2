"""Early-world base tiers from IdleOn AutoReview (TwoSpookyBoos/IdleOnAutoReviewBot), used with
its authors' permission (2026-11-06) as a base we change, not as is.

    python3 -I tools/autoreview-base/convert.py <AutoReview checkout>/mysite

Reads their tier tables (consts/progression_tiers.py) and their world mapping
(general/pinchy.py) as data only, with `ast.literal_eval` (their code is never run), and writes
crates/feat-idleon/crowd-base.txt: for World 1 to World 5, a table per mechanic in crowd.txt's
format. The crowd generator (`--example crowd`) takes a stage's column from here when there
aren't enough saves at that stage, and the owner edits the result in crowd.txt.

What changes on the way:
- every level above 1 is raised by LEVEL_FACTOR (the game is easier now than when they were set)
  and rounded down to a number a person would pick;
- their stages (W1, then early, mid and late W2...) become columns, each one stage earlier than
  theirs (their Early W2 is our Late W1); a tier asks for everything
  its earlier tiers did; a stage that asks the same as the one before is left out;
- totals with no item behind them (total stamp levels) are left out; counts ("13 star signs",
  "9 Megafeathers") become `@first N`, the first N in the game's order.

Special rows: `@first` (that many items in game order, found or level 1), `@first-max` (that
many, at their max), `!Name` (left out of both counts at that stage). A cell of 1e15 means
"at its max" (the page caps every target at the item's own max).
"""

import ast
import math
import os
import re
import sys

LEVEL_FACTOR = 1.25
MAX = 1e15
# Their stages (W1, Early W2 ... Late W5, Early W6), each moved one stage earlier: their tiers
# say what you need to start a stage (bubbles tier 1 is "minimum for finishing W2", mapped to
# Late W3), so what they expect by Early W2 is what a player finishes World 1 with.
STAGES = ["1m", "1l", "2e", "2m", "2l", "3e", "3m", "3l", "4e", "4m", "4l", "5e", "5m", "5l"]
STATUE_TIERS = {"Gold": 1, "Onyx": 2, "Zenith": 3}

# Their section (pinchy.Placements attribute) -> our mechanic key.
SECTIONS = {
    "STAMPS": "stamps", "BRIBES": "bribes", "STATUES": "statues", "STAR_SIGNS": "star-signs",
    "OWL": "owl", "VAULT": "upgrade-vault", "BUBBLES": "bubbles", "VIALS": "vials",
    "SIGILS": "sigils", "ISLANDS": "islands", "SALT_LICK": "salt-lick", "PRAYERS": "prayers",
    "COLLIDER": "atoms", "DIVINITY": "gods",
}
TABLES = {
    "STAMPS": "stamps_progressionTiers", "BRIBES": "bribes_progressionTiers",
    "STATUES": "statues_progressionTiers", "STAR_SIGNS": "starsigns_progressionTiers",
    "OWL": "owl_progressionTiers", "VAULT": "vault_progressionTiers",
    "BUBBLES": "bubbles_progressionTiers", "VIALS": "vials_progressionTiers",
    "SIGILS": "sigils_progressionTiers", "ISLANDS": "islands_progressionTiers",
    "SALT_LICK": "salt_lick_progression_tiers", "PRAYERS": "prayers_progressionTiers",
    "COLLIDER": "atoms_progressionTiers", "DIVINITY": "divinity_progressionTiers",
}


def literals(path):
    """Every top-level `name = <literal>` in a file."""
    out = {}
    for n in ast.parse(open(path, encoding="utf-8").read()).body:
        if isinstance(n, ast.Assign) and isinstance(n.targets[0], ast.Name):
            try:
                out[n.targets[0].id] = ast.literal_eval(n.value)
            except (ValueError, TypeError, SyntaxError):
                pass
    return out


def thresholds(path):
    """pinchy.Placements.sectionThresholds: section attribute -> tier per stage (W1 .. Early W6)."""
    tree = ast.parse(open(path, encoding="utf-8").read())
    for cls in tree.body:
        if isinstance(cls, ast.ClassDef) and cls.name == "Placements":
            for n in cls.body:
                if isinstance(n, ast.Assign) and getattr(n.targets[0], "id", "") == "sectionThresholds":
                    out = {}
                    for k, v in zip(n.value.keys, n.value.values):
                        tiers = []
                        for e in v.elts[: len(STAGES)]:
                            tiers.append(ast.literal_eval(e))
                        out[k.id] = tiers
                    return out
    raise SystemExit("no Placements.sectionThresholds in " + path)


def raise_level(v):
    """Their level, raised for an easier game, rounded down to a number a person would pick."""
    if v >= MAX or v <= 1:
        return v
    x = v * LEVEL_FACTOR
    if x < 20:
        return math.floor(x)
    if x < 100:
        return math.floor(x / 5) * 5
    p = 10 ** (math.floor(math.log10(x)) - 1)
    return math.floor(x / p) * p


def base_name(name):
    """'Hydrogen - Stamp Decreaser' -> 'Hydrogen'; 'Copper Corona (Copper Ore)' -> 'Copper Corona'."""
    name = name.split(" - ")[0]
    if name.endswith(")") and " (" in name:
        name = name[: name.rindex(" (")]
    return name.strip()


def one_tier(section, t):
    """One of their tiers as {row: level}; counts as `@first` / `@first-max`, exclusions as `!Name`."""
    need = {}

    def put(name, lv):
        name = base_name(name)
        need[name] = max(need.get(name, 0), lv)

    if section == "BUBBLES":
        for d in t[2:6]:
            for n, lv in d.items():
                put(n, lv)
        return need
    if not isinstance(t, dict):
        return need
    if section == "STAMPS":
        for cat, names in (t.get("Stamps") or {}).items():
            if cat == "Not Recommended":
                continue
            if cat == "Specific":
                for n, lv in names.items():
                    put(n, lv)
            else:
                for n in names:
                    # Two names run together where a comma is missing in their list
                    # ("Skelefish Stamp" "Cooked Meal Stamp").
                    for part in re.split(r"(?<=Stamp)(?=[A-Z])", n):
                        put(part, 1)
        for n, lv in (t.get("Specific") or {}).items():
            put(n, lv)
    elif section in ("BRIBES",):
        for names in t.values():
            for n in names:
                put(n, 1)
    elif section == "STATUES":
        level = STATUE_TIERS.get(t.get("MinStatueType", ""), 0)
        for n in t.get("SpecificTypes", []):
            put(n, level)
    elif section == "STAR_SIGNS":
        if t.get("UnlockedSigns"):
            need["@first"] = t["UnlockedSigns"]
        for n in t.get("SpecificSigns", []):
            put(n, 1)
    elif section == "OWL":
        if t.get("MegaFeathersOwned"):
            need["@first"] = t["MegaFeathersOwned"]
    elif section == "DIVINITY":
        if t.get("GodsUnlocked"):
            need["@first"] = t["GodsUnlocked"]
    elif section == "VAULT":
        for n in t.get("Include", []):
            put(n, MAX)
        for n, lv in (t.get("Levels") or {}).items():
            put(n, lv)
        if "Max Index" in t:
            need["@first-max"] = t["Max Index"] + 1
            for n in t.get("Exclude", []):
                need["!" + base_name(n)] = 1
    elif section == "VIALS":
        if t.get("Unlocked"):
            need["@first"] = t["Unlocked"]
            # Their counts leave out the Arcade's pickle vial ("excluding the Arcade Pickle").
            need["!Pickle Jar"] = 1
        for n in t.get("Recommended", []):
            put(n, MAX)
    elif section == "SIGILS":
        for n, lv in (t.get("Unlock") or {}).items():
            put(n, lv)
        for n, lv in (t.get("LevelUp") or {}).items():
            put(n, lv)
    elif section == "ISLANDS":
        for n in t.get("Islands", []):
            put(n, 1)
    elif section == "SALT_LICK":
        put(t["Upgrade"], t["Level"])
    elif section in ("PRAYERS",):
        for n, lv in t.items():
            put(n, lv)
    elif section == "COLLIDER":
        for n, lv in (t.get("Atoms") or {}).items():
            put(n, lv)
    return need


def tiers_of(section, table):
    """Tier number -> its own needs, from a dict keyed by tier or a list of [tier, ...] rows."""
    if isinstance(table, dict):
        return {k: one_tier(section, v) for k, v in table.items() if isinstance(k, int)}
    return {row[0]: one_tier(section, row) for row in table}


def cumulative(tiers, upto):
    """Everything tiers 1..upto ask for, the highest level of each."""
    out = {}
    for k in sorted(tiers):
        if k > upto:
            break
        for n, lv in tiers[k].items():
            out[n] = max(out.get(n, 0), lv)
    return out


def cell(v):
    if v is None or v <= 0:
        return "-"
    if v >= MAX:
        return "1e15"
    return f"{v:,.0f}" if v >= 1 else f"{v:.2f}"


def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    site = sys.argv[1]
    tables = literals(os.path.join(site, "consts", "progression_tiers.py"))
    stages = thresholds(os.path.join(site, "general", "pinchy.py"))
    here = os.path.dirname(os.path.abspath(__file__))
    out_path = os.path.join(here, "..", "..", "crates", "feat-idleon", "crowd-base.txt")
    out = [
        "# Early-world base tiers, from IdleOn AutoReview (TwoSpookyBoos), used with its authors'",
        "# permission as a base: levels raised x%s, their stages as our columns. Written by" % LEVEL_FACTOR,
        "# tools/autoreview-base/convert.py; edit crowd.txt, not this file.",
    ]
    for section, key in SECTIONS.items():
        table = tables.get(TABLES[section])
        if table is None or section not in stages:
            print(f"{section}: no table or no stages; left out", file=sys.stderr)
            continue
        tiers = tiers_of(section, table)
        cols = []
        last = None
        for i, tier in enumerate(stages[section]):
            need = {n: (raise_level(v) if not n.startswith(("@", "!")) else v) for n, v in cumulative(tiers, tier).items()}
            if not need or need == last:
                continue
            cols.append((STAGES[i], need))
            last = need
        if not cols:
            continue
        rows = []
        for _, need in cols:
            for n in need:
                if n not in rows:
                    rows.append(n)
        width = max(len(r) for r in rows) + 2
        out.append("")
        out.append(f"{key:<40}# {section.replace('_', ' ').title()} (AutoReview)")
        out.append("  " + "stages".ljust(width) + "".join(f"  {c:>6}" for c, _ in cols))
        for r in rows:
            out.append("  " + r.ljust(width) + "".join(f"  {cell(need.get(r)):>6}" for _, need in cols))
    with open(out_path, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print(f"wrote {out_path}", file=sys.stderr)


if __name__ == "__main__":
    main()
