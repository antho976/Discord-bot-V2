"""Banner directions for the RPG hub, drawn as the bot draws them (SVG -> resvg)."""
import math, random, sys, os

W, H = 932, 264
OUT = sys.argv[1] if len(sys.argv) > 1 else "."


def R(seed):
    return random.Random(seed)


def poly(pts, fill, extra=""):
    d = "M" + " L".join("%.1f,%.1f" % p for p in pts) + " Z"
    return f'<path d="{d}" fill="{fill}" {extra}/>'


def peaks(seed, base_y, spec, bottom=H):
    """A ridge of peaks over base_y; spec is (x, height, half width)."""
    r = R(seed)
    pts = [(0, bottom), (0, base_y)]
    for (x, h, w) in sorted(spec):
        pts += [(x - w, base_y - r.uniform(0, 5)),
                (x - w * 0.5, base_y - h * 0.5 + r.uniform(-4, 4)),
                (x - w * 0.15, base_y - h * 0.9),
                (x, base_y - h),
                (x + w * 0.2, base_y - h * 0.82),
                (x + w * 0.55, base_y - h * 0.42 + r.uniform(-4, 4)),
                (x + w, base_y - r.uniform(0, 5))]
    pts += [(W, base_y), (W, bottom)]
    return pts


def sparks(seed, n, x0, x1, y0, y1, cols, glow=True, rads=(1.1, 1.4, 1.8, 2.3)):
    r = R(seed)
    out = []
    for _ in range(n):
        x, y = r.uniform(x0, x1), r.uniform(y0, y1)
        rad, op, c = r.choice(rads), r.uniform(0.35, 0.95), r.choice(cols)
        out.append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{rad}" fill="{c}" opacity="{op:.2f}"/>')
        if glow and rad > 1.7:
            out.append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{rad*3.2:.1f}" fill="{c}" opacity="{op*0.22:.2f}" filter="url(#b3)"/>')
    return "\n".join(out)


DEFS = '''<filter id="b40" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="40"/></filter>
<filter id="b20" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="20"/></filter>
<filter id="b8" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="8"/></filter>
<filter id="b3" x="-100%" y="-100%" width="300%" height="300%"><feGaussianBlur stdDeviation="3"/></filter>
<clipPath id="r"><rect width="932" height="264" rx="12"/></clipPath>'''

# ------------------------------------------------------------------------------------- emblems
# Each is drawn upright around its own origin, lit from behind: a soft hot halo, a rim of light
# on one side, a dark metal body with a sideways gradient, then engraved detail.


def lit(d, rim, hot, metal):
    return (f'<path d="{d}" fill="{hot}" opacity="0.5" filter="url(#b3)"/>'
            f'<path d="{d}" fill="{rim}" transform="translate(-1.8,0)"/>'
            f'<path d="{d}" fill="url(#{metal})"/>')


def bident(rim, hot, metal, shaft=330):
    """Hades' bident: two leaf blades on a squared yoke, a ringed collar, a wrapped shaft.
    Origin at the yoke's base; blade tips at y=-146."""
    def blade(px):
        return (f"M{px},-146 C{px+8},-128 {px+11},-108 {px+8},-90 L{px+12},-82 L{px+5},-80 L{px+5},-70 "
                f"L{px-5},-70 L{px-5},-80 L{px-12},-82 L{px-8},-90 C{px-11},-108 {px-8},-128 {px},-146 Z")
    yoke = "M-23,-72 L-23,-22 L-12,0 L12,0 L23,-22 L23,-72 L13,-72 L13,-26 L6,-12 L-6,-12 L-13,-26 L-13,-72 Z"
    collar = "M-11,0 L11,0 L9,8 L-9,8 Z M-7,8 L7,8 L7,22 L-7,22 Z M-10,22 L10,22 L8,30 L-8,30 Z"
    body = f"M-4.5,30 L4.5,30 L4.5,{shaft} L-4.5,{shaft} Z"
    d = " ".join([blade(-18), blade(18), yoke, collar, body])
    wrap = " ".join(f"M-4.5,{y} L4.5,{y+5}" for y in range(70, min(shaft - 6, 140), 6))
    return (lit(d, rim, hot, metal)
            + f'<path d="M-18,-140 L-18,-88 M18,-140 L18,-88" stroke="{hot}" stroke-width="1.3" opacity="0.75"/>'
            + f'<path d="M-11,4 L11,4 M-7,15 L7,15 M-10,26 L10,26" stroke="{rim}" stroke-width="1" opacity="0.7"/>'
            + f'<path d="{wrap}" stroke="{rim}" stroke-width="1.5" opacity="0.6"/>')


def mjolnir(rim, hot, metal):
    """Thor's hammer, head up: a block head with flared faces and a knotwork band, a short wrapped
    haft, a pommel and a strap loop. Origin at the head's centre."""
    head = "M-60,-28 L60,-28 L78,-40 L78,40 L60,28 L-60,28 L-78,40 L-78,-40 Z"
    haft = "M-8,28 L8,28 L7,128 L-7,128 Z"
    pommel = "M-12,128 L12,128 L10,142 L-10,142 Z"
    d = " ".join([head, haft, pommel])
    knots = " ".join(f"M-16,{y} L16,{y+12} M16,{y} L-16,{y+12}" for y in (-26, -6, 14))
    wrap = " ".join(f"M-7,{y} L7,{y+6}" for y in range(36, 124, 7))
    return (f'<path d="M0,142 C-18,160 -14,184 0,186 C14,184 18,160 0,142" fill="none" stroke="{rim}" stroke-width="3.5" opacity="0.8"/>'
            + lit(d, rim, hot, metal)
            + f'<path d="M-16,-28 L-16,28 M16,-28 L16,28" stroke="{rim}" stroke-width="1.2" opacity="0.6"/>'
            + f'<path d="{knots}" stroke="{rim}" stroke-width="1.5" opacity="0.55"/>'
            + f'<path d="M-78,-40 L-78,40 M-60,-28 L60,-28" stroke="{hot}" stroke-width="1.6" opacity="0.85"/>'
            + f'<path d="{wrap}" stroke="{rim}" stroke-width="1.5" opacity="0.55"/>')


def owl(rim, hot, metal, eye):
    """Athena's owl, perched and facing you: rounded body, ear tufts, big lit eyes, folded wings.
    Origin at the body's centre; feet on y=72."""
    body = ("M0,-70 C22,-70 32,-62 38,-48 L50,-66 L46,-34 C54,-18 56,2 52,24 C46,54 26,72 0,74 "
            "C-26,72 -46,54 -52,24 C-56,2 -54,-18 -46,-34 L-50,-66 L-38,-48 C-32,-62 -22,-70 0,-70 Z")
    face = ("M0,-30 C10,-44 32,-46 40,-30 C44,-14 30,-4 16,-6 C8,-7 2,-12 0,-18 "
            "C-2,-12 -8,-7 -16,-6 C-30,-4 -44,-14 -40,-30 C-32,-46 -10,-44 0,-30 Z")
    wings = "M-46,-6 C-34,18 -30,44 -18,66 M46,-6 C34,18 30,44 18,66"
    chest = " ".join(f"M{x-5},{y} Q{x},{y+5} {x+5},{y}"
                     for y in (12, 24, 36, 48) for x in range(-18 + (y // 12 % 2) * 6, 20, 12))
    out = lit(body, rim, hot, metal)
    out += f'<path d="{face}" fill="{rim}" opacity="0.14"/>'
    out += f'<path d="{wings}" stroke="{rim}" stroke-width="1.6" fill="none" opacity="0.5"/>'
    out += f'<path d="{chest}" stroke="{rim}" stroke-width="1.3" fill="none" opacity="0.28"/>'
    for ex in (-19, 19):
        out += (f'<circle cx="{ex}" cy="-25" r="17" fill="{eye}" opacity="0.28" filter="url(#b3)"/>'
                f'<circle cx="{ex}" cy="-25" r="10" fill="{eye}"/>'
                f'<circle cx="{ex}" cy="-25" r="4.6" fill="#0B0816"/>'
                f'<circle cx="{ex-2}" cy="-27" r="1.4" fill="#FFFFFF" opacity="0.9"/>')
    out += f'<path d="M0,-19 L5,-9 L0,-1 L-5,-9 Z" fill="{hot}" opacity="0.85"/>'
    out += f'<path d="M-14,72 L-10,78 M-4,72 L-4,79 M14,72 L10,78 M4,72 L4,79" stroke="{hot}" stroke-width="2" stroke-linecap="round" opacity="0.7"/>'
    return out


def olive_branch(x, y, col, leaf, length=220, seed=4):
    """A branch running in from the right edge with olive leaves and a few olives."""
    r = R(seed)
    out = [f'<path d="M{x+length},{y+10} C{x+length*0.6},{y+2} {x+length*0.3},{y+8} {x},{y}" stroke="{col}" stroke-width="7" fill="none" stroke-linecap="round"/>']
    for i in range(9):
        t = 0.1 + i * 0.1
        px = x + length * t
        py = y + 6 * math.sin(t * 3) + 3
        side = -1 if i % 2 else 1
        ang = side * r.uniform(30, 60) + 180
        out.append(f'<ellipse cx="{px:.1f}" cy="{py + side*10:.1f}" rx="13" ry="4" transform="rotate({ang:.0f} {px:.1f} {py + side*10:.1f})" fill="{leaf}"/>')
    return "\n".join(out)


# ------------------------------------------------------------------------------------- the gods
GODS = {
    "hades": dict(word="HADES", accent="#F07A6A", ink="#F6EEE9", sub="#CDBDB6",
                  bar=("#C93A2B", "#F59A6E"), shade="#0B0506",
                  metal=("#4A1A1A", "#2A0C0E", "#150607"), rim="#E8503C", hot="#FFB08A"),
    "thor": dict(word="THOR", accent="#9CC2FF", ink="#F1F4FA", sub="#C3CDE0",
                 bar=("#5E8EE8", "#B9D4FF"), shade="#060A14",
                 metal=("#3A4A6A", "#1C2638", "#0D121D"), rim="#8FB8FF", hot="#E4EEFF"),
    "athena": dict(word="ATHENA", accent="#CDBBFF", ink="#F3F0FA", sub="#C9C2DE",
                   bar=("#8A72E0", "#D6C9FF"), shade="#09071A",
                   metal=("#4A4068", "#272040", "#141024"), rim="#B9A6FF", hot="#EFE8FF"),
}


def common_defs(g):
    c = GODS[g]
    m = c["metal"]
    return f'''<linearGradient id="shade" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="{c["shade"]}" stop-opacity="0.82"/><stop offset="0.42" stop-color="{c["shade"]}" stop-opacity="0.45"/><stop offset="0.62" stop-color="{c["shade"]}" stop-opacity="0"/></linearGradient>
<linearGradient id="bar" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="{c["bar"][0]}"/><stop offset="1" stop-color="{c["bar"][1]}"/></linearGradient>
<linearGradient id="metal" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="{m[0]}"/><stop offset="0.35" stop-color="{m[1]}"/><stop offset="1" stop-color="{m[2]}"/></linearGradient>'''


# ------------------------------------------------------------------------------------- A: scenes
def scene_hades():
    c = GODS["hades"]
    far = peaks(3, 214, [(90, 52, 120), (300, 38, 110), (520, 62, 140), (790, 70, 150)])
    mid = peaks(7, 232, [(40, 44, 90), (210, 30, 110), (440, 22, 90), (690, 40, 120), (900, 52, 100)])
    r = R(11)
    ceil = [(0, 0)]
    x = 0
    while x < W:
        w = r.uniform(18, 44)
        h = r.uniform(6, 30) if r.random() > 0.3 else r.uniform(34, 58)
        ceil += [(x, r.uniform(2, 10)), (x + w * 0.45, h), (x + w * 0.55, h - 2), (x + w, r.uniform(2, 10))]
        x += w
    ceil += [(W, 0)]
    rr = R(5)
    ripples = "".join(
        f'<path d="M{cx-w/2:.1f},{y:.1f} L{cx+w/2:.1f},{y:.1f}" stroke="#F0704E" stroke-width="1.1" opacity="{0.55 - i*0.03:.2f}"/>'
        for i in range(16)
        for (y, cx, w) in [(230 + i * 2.2 + rr.uniform(-0.6, 0.6), 600 + rr.uniform(-60, 60), rr.uniform(30, 150) * (1 - i / 22))])
    gate = '''<g transform="translate(566,196) scale(0.62)">
  <path d="M-8,-36 L50,-36 L50,30 L-8,30 Z" fill="#FF8A52" opacity="0.55" filter="url(#b8)"/>
  <rect x="-2" y="-30" width="46" height="60" fill="#D9502F"/>
  <rect x="6" y="-20" width="30" height="50" fill="#FFB27A" opacity="0.45" filter="url(#b3)"/>
  <path d="M-16,-30 L21,-50 L58,-30 Z" fill="#160608"/>
  <rect x="-16" y="-30" width="74" height="6" fill="#160608"/>
  <rect x="-12" y="-24" width="10" height="56" fill="#160608"/><rect x="44" y="-24" width="10" height="56" fill="#160608"/>
  <rect x="16" y="-24" width="10" height="56" fill="#160608"/>
  <path d="M-26,32 L68,32 L74,40 L-32,40 Z" fill="#160608"/>
</g>'''
    boat = '''<g transform="translate(452,238) scale(1.25)">
  <path d="M-34,-2 Q0,10 38,-4 L34,2 Q0,12 -30,4 Z" fill="#090304"/>
  <path d="M-6,-3 L-2,-25 Q2,-31 6,-25 L8,-3 Z" fill="#090304"/>
  <path d="M-16,8 L22,-46" stroke="#090304" stroke-width="2.2" stroke-linecap="round"/>
  <circle cx="-26" cy="-12" r="2.4" fill="#FFB27A"/>
  <circle cx="-26" cy="-12" r="9" fill="#FF8A52" opacity="0.45" filter="url(#b3)"/>
  <path d="M-30,-3 L-26,-12" stroke="#090304" stroke-width="1.6"/>
</g>'''
    return f'''<defs>{common_defs("hades")}
  <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#14080A"/><stop offset="0.62" stop-color="#2E0C10"/><stop offset="1" stop-color="#14070A"/></linearGradient>
  <linearGradient id="water" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1C080A"/><stop offset="1" stop-color="#0A0405"/></linearGradient>
</defs>
<rect width="932" height="264" fill="url(#sky)"/>
<ellipse cx="600" cy="196" rx="330" ry="80" fill="#B42A1C" opacity="0.55" filter="url(#b40)"/>
<ellipse cx="600" cy="186" rx="120" ry="34" fill="#E2502E" opacity="0.45" filter="url(#b20)"/>
{poly(far, "#3E1013")}
{gate}
{poly(mid, "#200709")}
<rect y="226" width="932" height="38" fill="url(#water)"/>
<ellipse cx="600" cy="236" rx="140" ry="10" fill="#E2502E" opacity="0.35" filter="url(#b8)"/>
{ripples}
{boat}
{poly(ceil, "#090304")}
{sparks(21, 26, 380, 930, 60, 230, ["#FF8A52", "#F0603E", "#FFB27A"])}
<ellipse cx="808" cy="86" rx="86" ry="90" fill="#E2402C" opacity="0.32" filter="url(#b20)"/>
<g transform="translate(836,178) rotate(-12) scale(1.12)">{bident(c["rim"], c["hot"], "metal")}</g>
<rect width="932" height="264" fill="url(#shade)"/>'''


def bolt(seed, x0, y0, x1, y1, steps=7, jitter=18):
    r = R(seed)
    pts = [(x0, y0)]
    for i in range(1, steps):
        t = i / steps
        pts.append((x0 + (x1 - x0) * t + r.uniform(-jitter, jitter), y0 + (y1 - y0) * t))
    pts.append((x1, y1))
    return " ".join("%.1f,%.1f" % p for p in pts)


def scene_thor():
    c = GODS["thor"]
    far = peaks(13, 200, [(120, 70, 90), (330, 96, 80), (520, 60, 110), (720, 84, 90)])
    mid = peaks(17, 226, [(60, 40, 120), (260, 54, 90), (470, 34, 100), (660, 46, 120), (880, 38, 90)])
    rr = R(31)
    rain = "".join(f'<path d="M{x:.0f},{y:.0f} l-7,22" stroke="#B9D4FF" stroke-width="1" opacity="{rr.uniform(0.08, 0.22):.2f}"/>'
                   for x, y in [(rr.uniform(300, 960), rr.uniform(20, 230)) for _ in range(70)])
    clouds = "".join(
        f'<ellipse cx="{x:.0f}" cy="{y:.0f}" rx="{w:.0f}" ry="{h:.0f}" fill="{col}" opacity="{op}" filter="url(#b20)"/>'
        for (x, y, w, h, col, op) in [(160, 10, 220, 50, "#2C3E66", 0.9), (480, 0, 260, 56, "#24355A", 0.9),
                                     (780, 14, 240, 60, "#33497A", 0.8), (640, 40, 160, 30, "#1A2747", 0.9)])
    waves = "".join(f'<path d="M{x:.0f},{y:.1f} q10,-3 20,0 t20,0" stroke="#7FA6E8" stroke-width="1.1" fill="none" opacity="{op:.2f}"/>'
                    for x, y, op in [(rr.uniform(0, 900), 234 + i * 2.6, 0.4 - i * 0.03) for i in range(12)])
    ship = '''<g transform="translate(470,236)">
  <path d="M-44,-6 Q-40,6 -20,8 L24,8 Q42,6 46,-6 Q50,-14 54,-20 L50,-20 Q44,-10 30,-6 L-30,-6 Q-42,-8 -48,-20 L-52,-20 Q-48,-14 -44,-6 Z" fill="#070B14"/>
  <path d="M-2,-6 L-2,-50" stroke="#070B14" stroke-width="2.4"/>
  <path d="M-24,-46 L22,-46 L18,-14 L-20,-14 Z" fill="#0B1220"/>
  <path d="M-24,-46 L22,-46" stroke="#070B14" stroke-width="2"/>
  <path d="M-20,-38 L18,-38 M-20,-30 L18,-30 M-20,-22 L18,-22" stroke="#9CC2FF" stroke-width="1" opacity="0.18"/>
</g>'''
    b = bolt(41, 912, -10, 846, 92, 6, 16)
    branch = bolt(43, 884, 30, 940, 70, 3, 8)
    return f'''<defs>{common_defs("thor")}
  <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#121B33"/><stop offset="0.65" stop-color="#22345C"/><stop offset="1" stop-color="#0E1528"/></linearGradient>
  <linearGradient id="water" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#16213C"/><stop offset="1" stop-color="#080C18"/></linearGradient>
</defs>
<rect width="932" height="264" fill="url(#sky)"/>
<ellipse cx="820" cy="90" rx="260" ry="110" fill="#5E8EE8" opacity="0.32" filter="url(#b40)"/>
{clouds}
{poly(far, "#1C2A4A")}
{poly(mid, "#111A30")}
<rect y="228" width="932" height="36" fill="url(#water)"/>
{waves}
{ship}
{rain}
<g fill="none" stroke-linejoin="round" stroke-linecap="round">
  <polyline points="{b}" stroke="#9CC2FF" stroke-width="16" opacity="0.18" filter="url(#b3)"/>
  <polyline points="{b}" stroke="#B9D4FF" stroke-width="6" opacity="0.45"/>
  <polyline points="{b}" stroke="#F4F8FF" stroke-width="2.6"/>
  <polyline points="{branch}" stroke="#B9D4FF" stroke-width="3" opacity="0.4"/>
  <polyline points="{branch}" stroke="#F4F8FF" stroke-width="1.3"/>
</g>
<ellipse cx="814" cy="102" rx="96" ry="72" fill="#9CC2FF" opacity="0.35" filter="url(#b20)"/>
<g transform="translate(812,106) rotate(-16) scale(0.8)">{mjolnir(c["rim"], c["hot"], "metal")}</g>
<rect width="932" height="264" fill="url(#shade)"/>'''


def scene_athena():
    c = GODS["athena"]
    rr = R(51)
    stars = "".join(f'<circle cx="{rr.uniform(330, 932):.0f}" cy="{rr.uniform(8, 150):.0f}" r="{rr.choice([0.8, 1.1, 1.5, 2.0])}" fill="#EEE8FF" opacity="{rr.uniform(0.3, 0.9):.2f}"/>' for _ in range(60))
    far = peaks(23, 212, [(80, 40, 140), (300, 30, 120), (560, 26, 140), (820, 36, 140)])
    # the Acropolis: a flat-topped hill with the Parthenon on it, lamp-lit
    hill = [(380, 264), (380, 226), (430, 204), (470, 196), (640, 194), (690, 202), (740, 226), (740, 264)]
    cols = "".join(f'<rect x="{486 + i*14}" y="160" width="6" height="34" fill="#100C22"/>' for i in range(10))
    parthenon = f'''<path d="M476,160 L620,160 L548,140 Z" fill="#100C22"/>
<rect x="472" y="154" width="152" height="8" fill="#100C22"/>
<rect x="480" y="160" width="136" height="34" fill="#F2C47A" opacity="0.55"/>
<rect x="480" y="160" width="136" height="34" fill="#FFD9A0" opacity="0.4" filter="url(#b8)"/>
{cols}
<rect x="468" y="192" width="160" height="6" fill="#100C22"/>'''
    return f'''<defs>{common_defs("athena")}
  <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#0F0C22"/><stop offset="0.7" stop-color="#2A2250"/><stop offset="1" stop-color="#120E26"/></linearGradient>
</defs>
<rect width="932" height="264" fill="url(#sky)"/>
<ellipse cx="560" cy="200" rx="300" ry="70" fill="#6A56C8" opacity="0.35" filter="url(#b40)"/>
<ellipse cx="548" cy="176" rx="110" ry="36" fill="#F2C47A" opacity="0.22" filter="url(#b20)"/>
{stars}
<circle cx="700" cy="56" r="22" fill="#EEE8FF" opacity="0.9"/>
<circle cx="710" cy="50" r="20" fill="#191435"/>
<circle cx="700" cy="56" r="44" fill="#CDBBFF" opacity="0.18" filter="url(#b8)"/>
{poly(far, "#221C44")}
{poly(hill, "#151030")}
{parthenon}
{poly(peaks(29, 240, [(120, 22, 160), (340, 14, 120), (860, 30, 140)]), "#0E0B20")}
<ellipse cx="830" cy="120" rx="100" ry="90" fill="#9C86F0" opacity="0.3" filter="url(#b20)"/>
{olive_branch(760, 214, "#0B0918", "#1A1534", 220)}
<g transform="translate(838,136) scale(1.15)">{owl(c["rim"], c["hot"], "metal", "#F2C47A")}</g>
<rect width="932" height="264" fill="url(#shade)"/>'''


# ------------------------------------------------------------------------------------- B: seals
def seal(g):
    """A big struck coin bleeding off the right edge, the god's emblem raised on it."""
    c = GODS[g]
    cx, cy, rad = 812, 132, 150
    beads = "".join(
        f'<circle cx="{cx + (rad-14)*math.cos(a):.1f}" cy="{cy + (rad-14)*math.sin(a):.1f}" r="2.6" fill="{c["rim"]}" opacity="0.55"/>'
        for a in [i * 2 * math.pi / 72 for i in range(72)])
    if g == "hades":
        em = f'<g transform="translate({cx},{cy+20}) scale(0.92)">{bident(c["rim"], c["hot"], "metal", shaft=92)}</g>'
        bg = ("#120709", "#2A0B0F", "#B42A1C")
    elif g == "thor":
        em = f'<g transform="translate({cx},{cy-30}) rotate(-12) scale(0.72)">{mjolnir(c["rim"], c["hot"], "metal")}</g>'
        bg = ("#0C1324", "#1E2E52", "#5E8EE8")
    else:
        em = f'<g transform="translate({cx},{cy-4}) scale(0.92)">{owl(c["rim"], c["hot"], "metal", "#F2C47A")}</g>'
        bg = ("#0E0B20", "#272048", "#6A56C8")
    return f'''<defs>{common_defs(g)}
  <linearGradient id="sky" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="{bg[0]}"/><stop offset="1" stop-color="{bg[1]}"/></linearGradient>
  <radialGradient id="coin" cx="0.38" cy="0.32" r="0.8"><stop offset="0" stop-color="{c["metal"][0]}"/><stop offset="0.6" stop-color="{c["metal"][1]}"/><stop offset="1" stop-color="{c["metal"][2]}"/></radialGradient>
</defs>
<rect width="932" height="264" fill="url(#sky)"/>
<circle cx="{cx}" cy="{cy}" r="{rad+30}" fill="{bg[2]}" opacity="0.35" filter="url(#b40)"/>
<circle cx="{cx}" cy="{cy}" r="{rad}" fill="url(#coin)"/>
<circle cx="{cx}" cy="{cy}" r="{rad-1}" fill="none" stroke="{c["rim"]}" stroke-width="2" opacity="0.8"/>
<circle cx="{cx}" cy="{cy}" r="{rad-26}" fill="none" stroke="{c["rim"]}" stroke-width="1.2" opacity="0.45"/>
{beads}
<circle cx="{cx}" cy="{cy}" r="{rad-30}" fill="{c["shade"]}" opacity="0.35"/>
{em}
{sparks(61, 18, 420, 650, 20, 250, [c["accent"]], rads=(0.9, 1.2, 1.6))}
<rect width="932" height="264" fill="url(#shade)"/>'''


# ------------------------------------------------------------------------------------- text
def text_block(g, name, level, power, xp10, title, mode):
    """mode 'epithet': the title on its own line under the name, after a small diamond.
    mode 'inline': the title at the end of the stats line, in the accent."""
    c = GODS[g]
    eyebrow = f"MIDGARD · SWORN TO {c['word']}"
    xp_w = 360 * min(xp10, 10) / 10
    out = [f'<text x="40" y="60" font-family="Geist" font-weight="600" font-size="20" letter-spacing="2.4" fill="{c["accent"]}">{eyebrow}</text>',
           f'<text x="38" y="118" font-family="Geist" font-weight="600" font-size="54" letter-spacing="-0.8" fill="{c["ink"]}">{name}</text>']
    stats = (f'Level <tspan font-weight="600" fill="{c["ink"]}">{level}</tspan>  ·  '
             f'<tspan font-weight="600" fill="{c["ink"]}">{power}</tspan> Power')
    if mode == "epithet" and title:
        out.append(f'<path d="M48,141 L55,149 L48,157 L41,149 Z" fill="{c["accent"]}"/>')
        out.append(f'<text x="66" y="158" font-family="Geist" font-size="26" fill="{c["ink"]}" fill-opacity="0.9">{title}</text>')
        out.append(f'<text x="40" y="204" font-family="Geist" font-size="26" fill="{c["sub"]}">{stats}</text>')
        by = 221
    else:
        if mode == "inline" and title:
            stats += f'  ·  <tspan fill="{c["accent"]}">{title}</tspan>'
        out.append(f'<text x="40" y="166" font-family="Geist" font-size="26" fill="{c["sub"]}">{stats}</text>')
        by = 190
    out.append(f'<rect x="40" y="{by}" width="360" height="6" rx="3" fill="#FFFFFF" fill-opacity="0.14"/>')
    out.append(f'<rect x="40" y="{by}" width="{xp_w:.0f}" height="6" rx="3" fill="url(#bar)"/>')
    return "\n".join(out)


def banner(g, scene, **kw):
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="932" height="264" viewBox="0 0 932 264">
<defs>{DEFS}</defs>
<g clip-path="url(#r)">
{scene}
{text_block(g, **kw)}
</g>
</svg>'''


if __name__ == "__main__":
    P = dict(name="Picasso Antho", level=7, power=140, xp10=3, title="Siren-deaf")
    scenes = {"hades": scene_hades, "thor": scene_thor, "athena": scene_athena}
    for g, fn in scenes.items():
        open(os.path.join(OUT, f"{g}_a.svg"), "w").write(banner(g, fn(), mode="epithet", **P))
        open(os.path.join(OUT, f"{g}_b.svg"), "w").write(banner(g, seal(g), mode="inline", **P))
    # The scene art alone (defs and picture, no words), as the bot embeds it.
    if len(sys.argv) > 2:
        for g, fn in scenes.items():
            open(os.path.join(sys.argv[2], f"{g}.svg"), "w").write(
                f"<!-- Drawn by gen.py; edit that, not this. -->\n<defs>{DEFS}</defs>\n{fn()}\n")
