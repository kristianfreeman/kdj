"""kdj enclosure, slab layout, in build123d: the real parts in a printed box.

The Raspberry Pi 4 is its published STEP model (parts/, fetched by
cad/fetch-parts.sh); the 5" DSI screen is modelled from its manual (V1.4,
section 3); the InnoMaker HiFi DAC HAT from its listing (its jack positions
are a guess until measured: search MEASURE). The 15-pin ribbon runs from the
Pi's DISPLAY connector to the screen's.

Run in the kdj-cad container (tools/enclosure-live.sh does, on every save):
writes live/*.glb (coloured, for viewer.html), live/parts.json, live/report.json
(sizes, and every pair of parts that collide, with the overlap exported as
live/collisions.glb), and out/*.stl / out/*.step for printing.

Coordinates: mm, Z up, the box's front-left-bottom corner at the origin; the
screen faces up as the lid; the back wall is at max Y.
"""

import json
import os
import time
from itertools import combinations
from pathlib import Path

from build123d import (
    Align,
    Box,
    Color,
    Compound,
    Cylinder,
    Pos,
    Rot,
    Shape,
    export_brep,
    export_gltf,
    export_step,
    export_stl,
    import_brep,
    import_step,
)

HERE = Path(__file__).resolve().parent.parent
LIVE = HERE / "live"
OUT = HERE / "out"
PARTS = HERE / "parts"
MIN = (Align.MIN, Align.MIN, Align.MIN)
CTR_XY = (Align.CENTER, Align.CENTER, Align.MIN)

# --- The screen: 5" DSI display, from its manual --------------------------
SCREEN_W, SCREEN_D = 121.11, 77.93          # module (PCB) outline
LENS_W, LENS_D = 120.5, 75.65               # cover glass
VA_W, VA_D = 109.20, 66.05                  # visible window in the glass print
VA_RIGHT, VA_TOP = 5.65, 2.70               # window from the glass's right / top edge
ACTIVE_W, ACTIVE_D = 108.0, 64.8            # lit pixels
LENS_T = 0.25 + 0.20 + 0.70                 # PET lens + adhesive + touch glass
TFT_T, PASTE_T, SPCB_T, NUT_H = 3.0, 1.5, 1.6, 4.0
SCREEN_T = LENS_T + TFT_T + PASTE_T + SPCB_T  # 7.25: glass to the back of its board
CORNER_HOLES = (111.11, 67.93)              # centred: 5 mm in from each edge
# The Pi's holes on the screen (it can mount there): 58 x 49, the left pair
# 38.11 from the left corner hole, the lower pair 16.5 above the bottom one.
SCREEN_PI_HOLES = [(5 + 38.11 + dx, 5 + 16.5 + dy) for dx in (0, 58) for dy in (0, 49)]
FPC_FROM_RIGHT = 13.48                      # ribbon connector from the right edge
FPC_LEN = 46.01                             # it stands on end, centred on this run

# --- The Pi 4 (its STEP model) ---------------------------------------------
PI_W, PI_D, PI_T = 85.0, 56.0, 1.6
PI_HOLES = [(3.5, 3.5), (61.5, 3.5), (3.5, 52.5), (61.5, 52.5)]
DSI_ON_PI = (2.5, 28.0)                     # DISPLAY connector, Pi coordinates (approx.)
STANDOFF_H = 4.0

# Ports, Pi coordinates: name, edge ("s" = USB-C edge, "e" = USB-A end),
# centre along that edge, width, height, cut through the wall?
USB_A = True
PI_PORTS = [
    ("USB-C", "s", 11.2, 9.0, 3.3, True),
    ("HDMI 0", "s", 26.0, 7.5, 3.5, False),
    ("HDMI 1", "s", 39.5, 7.5, 3.5, False),
    ("Audio", "s", 53.5, 6.0, 6.0, False),
    ("USB 3", "e", 9.0, 13.5, 16.0, USB_A),
    ("USB 2", "e", 27.0, 13.5, 16.0, USB_A),
    ("Ethernet", "e", 45.75, 16.0, 13.5, False),
]

# --- The DAC HAT (InnoMaker HiFi DAC) -------------------------------------
HAT_W, HAT_D, HAT_T = 65.0, 56.5, 1.6
HAT_GAP = 11.5                              # Pi board top to HAT board bottom
# Jacks along the HAT's edge over the Pi's USB-C edge: name, centre (Pi x),
# diameter, centre height above the HAT board, reach past the edge. MEASURE.
HAT_JACKS = [("RCA L", 18.0, 8.5, 6.5, 9.0), ("RCA R", 32.0, 8.5, 6.5, 9.0), ("3.5 mm", 48.0, 6.0, 4.0, 3.0)]
JACK_BODY = (10.0, 12.0, 12.0)
CUT_JACKS = ("RCA L", "RCA R")

# --- The box ---------------------------------------------------------------
WALL, FLOOR, GAP, LIP, CLEARANCE, PORT_GAP = 2.4, 2.0, 0.4, 2.5, 2.0, 1.0
IN_W = max(SCREEN_W, PI_W + 10) + 2 * GAP
IN_D = max(SCREEN_D, PI_D + 10) + 2 * GAP
OUT_W, OUT_D = IN_W + 2 * WALL, IN_D + 2 * WALL
BOARD_Z = FLOOR + STANDOFF_H                 # Pi board bottom
STACK_TOP = BOARD_Z + PI_T + HAT_GAP + HAT_T + JACK_BODY[2]
BASE_H = STACK_TOP + CLEARANCE + NUT_H       # the screen's back (and nuts) sit on top
LID_H = SCREEN_T + WALL
PI_X = WALL + (IN_W - PI_W) / 2
PI_Y = OUT_D - WALL - PI_D - 1.5             # USB-C edge near the back wall
SCREEN_X, SCREEN_Y = (OUT_W - SCREEN_W) / 2, (OUT_D - SCREEN_D) / 2
SCREEN_Z = BASE_H                            # the back of the screen's board


def placed(x, y, z=0.0):
    """A point in the Pi's coordinates, in the box's: the Pi lies turned
    180 degrees, its USB-C edge to the back wall, its USB end to the left."""
    return (PI_X + PI_W - x, PI_Y + PI_D - y, z)


def tag(shape, label, color):
    shape.label = label
    shape.color = Color(color)
    return shape


# --- Parts ------------------------------------------------------------------

def pi_model():
    """The Pi 4 STEP, upright, its board's corner at the origin and its top
    at z = PI_T, then turned and placed in the box. Cached as BREP."""
    cache = PARTS / "pi4.brep"
    src = PARTS / "raspberry_pi_4_model_b.step"
    if cache.exists() and cache.stat().st_mtime > src.stat().st_mtime:
        pi = import_brep(str(cache))
    else:
        pi = import_step(str(src))
        export_brep(pi, str(cache))
    # Its file lies on its side: board in XZ, components toward +Y.
    pi = Rot(90, 0, 0) * pi
    pi = Pos(0, 0, PI_T) * pi
    pi = Pos(*placed(0, 0, BOARD_Z)) * Rot(0, 0, 180) * pi
    return tag(pi, "Raspberry Pi 4", "#2e7d32")


def dac_model():
    hz = BOARD_Z + PI_T + HAT_GAP
    parts = []
    board = Pos(PI_X + PI_W - HAT_W, PI_Y, hz) * Box(HAT_W, HAT_D, HAT_T, align=MIN)
    for hx, hy in PI_HOLES:
        x, y, _ = placed(hx, hy)
        board -= Pos(x, y, hz - 1) * Cylinder(1.35, HAT_T + 2, align=CTR_XY)
    parts.append(tag(board, "DAC board", "#1b5e20"))
    # Stacking socket on the Pi's header (it ends above the header's pins).
    sx, sy, _ = placed(7.1 + 50.8, PI_D - 3.5 + 2.5)
    parts.append(tag(Pos(sx, sy, hz - 3.0) * Box(50.8, 5.0, 3.0, align=MIN), "DAC socket", "#111111"))
    cx, cy, _ = placed(30, 28)
    parts.append(tag(Pos(cx, cy, hz + HAT_T) * Box(9.7, 6.4, 1.2, align=CTR_XY), "PCM5122", "#222222"))
    for name, jx, d, cz, reach in HAT_JACKS:
        x, edge_y, _ = placed(jx, 0)
        body = Pos(x, edge_y - JACK_BODY[1], hz + HAT_T) * Box(JACK_BODY[0], JACK_BODY[1], JACK_BODY[2], align=(Align.CENTER, Align.MIN, Align.MIN))
        barrel = Pos(x, edge_y, hz + HAT_T + cz) * Rot(-90, 0, 0) * Cylinder(d / 2, reach, align=CTR_XY)
        parts.append(tag(body, f"{name} body", "#151515"))
        parts.append(tag(barrel, name, "#c9a227" if "RCA" in name else "#9e9e9e"))
    return parts


def screen_model():
    """Face up: glass on top, its board below, the nuts hanging under that."""
    x0, y0, z = SCREEN_X, SCREEN_Y, SCREEN_Z
    parts = []
    pcb = Pos(x0, y0, z) * Box(SCREEN_W, SCREEN_D, SPCB_T, align=MIN)
    holes = [(SCREEN_W / 2 + sx * CORNER_HOLES[0] / 2, SCREEN_D / 2 + sy * CORNER_HOLES[1] / 2) for sx in (-1, 1) for sy in (-1, 1)]
    for hx, hy in holes + SCREEN_PI_HOLES:
        pcb -= Pos(x0 + hx, y0 + hy, z - 1) * Cylinder(1.35, SPCB_T + 2, align=CTR_XY)
    parts.append(tag(pcb, "Screen board", "#1e4fa3"))
    parts.append(tag(Pos(x0 + 2, y0 + 7, z + SPCB_T) * Box(SCREEN_W - 4, SCREEN_D - 9, PASTE_T, align=MIN), "Screen frame", "#3a3a3a"))
    parts.append(tag(Pos(x0 + 0.3, y0 + 1.1, z + SPCB_T + PASTE_T) * Box(LENS_W - 0.6, LENS_D - 1.2, TFT_T, align=MIN), "TFT", "#1a1a1a"))
    lens_x, lens_y = x0 + (SCREEN_W - LENS_W) / 2, y0 + (SCREEN_D - LENS_D)
    lens_z = z + SPCB_T + PASTE_T + TFT_T
    parts.append(tag(Pos(lens_x, lens_y, lens_z) * Box(LENS_W, LENS_D, LENS_T, align=MIN), "Cover glass", "#0d0d0f"))
    # The lit area, a hair proud of the glass so it reads on screen.
    ax = lens_x + LENS_W - VA_RIGHT - VA_W / 2
    ay = lens_y + LENS_D - VA_TOP - VA_D / 2
    parts.append(tag(Pos(ax, ay, lens_z + LENS_T) * Box(ACTIVE_W, ACTIVE_D, 0.05, align=CTR_XY), "Lit area", "#26324a"))
    for hx, hy in holes + SCREEN_PI_HOLES:
        nut = Pos(x0 + hx, y0 + hy, z - NUT_H) * Cylinder(2.5, NUT_H, align=CTR_XY)
        nut -= Pos(x0 + hx, y0 + hy, z - NUT_H - 1) * Cylinder(1.25, NUT_H + 2, align=CTR_XY)
        parts.append(tag(nut, "Brass nut", "#b8902e"))
    # Underside parts: the ribbon connector (on end), backlight key, fan header.
    fx = x0 + SCREEN_W - FPC_FROM_RIGHT
    parts.append(tag(Pos(fx, y0 + SCREEN_D / 2, z - 2.5) * Box(5.0, 20.0, 2.5, align=CTR_XY), "DSI connector", "#202020"))
    parts.append(tag(Pos(x0 + 5 + 15.03, y0 + SCREEN_D - 3, z - 3.5) * Box(6.0, 3.5, 3.5, align=MIN), "Backlight key", "#c0c0c0"))
    parts.append(tag(Pos(x0 + SCREEN_W - 9, y0 + 14, z - 6) * Box(5.0, 2.5, 6.0, align=MIN), "Fan header", "#202020"))
    return parts


def ribbon_model():
    """The 15-pin, 1 mm ribbon (16 mm wide), as flat runs. The Pi's DISPLAY
    connector sits under the HAT, 2.5 mm in from the board's edge, so the
    ribbon folds flat just above it, runs out under the HAT past that edge,
    then rises beside the stack to the screen's connector, which is a few mm
    beyond the Pi's edge."""
    w, t = 16.0, 0.15
    px, py, _ = placed(*DSI_ON_PI)
    leave = BOARD_Z + PI_T + 5.5                       # top of the Pi's connector
    fold = BOARD_Z + PI_T + 7.0                        # under the HAT (11.5 up)
    fx = SCREEN_X + SCREEN_W - FPC_FROM_RIGHT
    runs = [
        Pos(px - t / 2, py - w / 2, leave) * Box(t, w, fold - leave, align=MIN),
        Pos(min(px, fx), py - w / 2, fold) * Box(abs(fx - px), w, t, align=MIN),
        Pos(fx - t / 2, py - w / 2, fold) * Box(t, w, SCREEN_Z - 2.5 - fold, align=MIN),
    ]
    return [tag(r, "Ribbon", "#e0e0e0") for r in runs]


# --- The printed box ---------------------------------------------------------

def port_cutouts():
    cuts = []
    for name, edge, c, w, h, show in PI_PORTS:
        if not show:
            continue
        z = BOARD_Z + PI_T - PORT_GAP
        if edge == "s":
            x, _, _ = placed(c + w / 2 + PORT_GAP, 0)
            cuts.append(Pos(x, OUT_D - WALL - 1, z) * Box(w + 2 * PORT_GAP, WALL + 2, h + 2 * PORT_GAP, align=MIN))
        else:
            _, y, _ = placed(0, c + w / 2 + PORT_GAP)
            cuts.append(Pos(-1, y, z) * Box(WALL + 2, w + 2 * PORT_GAP, h + 2 * PORT_GAP, align=MIN))
    hz = BOARD_Z + PI_T + HAT_GAP + HAT_T
    for name, jx, d, cz, _ in HAT_JACKS:
        if name in CUT_JACKS:
            x, _, _ = placed(jx, 0)
            cuts.append(Pos(x, OUT_D - WALL - 1, hz + cz) * Rot(-90, 0, 0) * Cylinder(d / 2 + PORT_GAP, WALL + 2, align=CTR_XY))
    return cuts


def base_model():
    base = Box(OUT_W, OUT_D, BASE_H, align=MIN) - Pos(WALL, WALL, FLOOR) * Box(IN_W, IN_D, BASE_H, align=MIN)
    for c in port_cutouts():
        base -= c
    for i in range(8):                              # floor vents under the Pi
        x, y, _ = placed(PI_W - 14 - i * 8, PI_D - 12)
        base -= Pos(x, y, -1) * Box(3, PI_D - 24, FLOOR + 2, align=MIN)
    for i in range(8):                              # right wall vents
        base -= Pos(OUT_W - WALL - 1, WALL + 10 + i * 7, FLOOR + 4) * Box(WALL + 2, 3, BASE_H * 0.5, align=MIN)
    for hx, hy in PI_HOLES:                         # Pi standoffs
        x, y, _ = placed(hx, hy)
        base += Pos(x, y, FLOOR) * Cylinder(3, STANDOFF_H, align=CTR_XY) - Pos(x, y, FLOOR) * Cylinder(1.25, STANDOFF_H + 1, align=CTR_XY)
    for x in (WALL + 4, OUT_W - WALL - 4):          # corner posts for the lid
        for y in (WALL + 4, OUT_D - WALL - 4):
            base += Pos(x, y, 0) * Cylinder(3.5, BASE_H, align=CTR_XY) - Pos(x, y, BASE_H - 8) * Cylinder(2.1, 9, align=CTR_XY)
    return tag(base, "Base (printed)", "#2f3440")


def lid_model():
    z = BASE_H
    lid = Pos(0, 0, z) * Box(OUT_W, OUT_D, LID_H, align=MIN)
    lid -= Pos(SCREEN_X - GAP, SCREEN_Y - GAP, z - 1) * Box(SCREEN_W + 2 * GAP, SCREEN_D + 2 * GAP, SCREEN_T + 1, align=MIN)
    lens_x, lens_y = SCREEN_X + (SCREEN_W - LENS_W) / 2, SCREEN_Y + (SCREEN_D - LENS_D)
    ax = lens_x + LENS_W - VA_RIGHT - VA_W / 2
    ay = lens_y + LENS_D - VA_TOP - VA_D / 2
    lid -= Pos(ax, ay, z + SCREEN_T - 1) * Box(VA_W, VA_D, WALL + 2, align=CTR_XY)
    for x in (WALL + 4, OUT_W - WALL - 4):
        for y in (WALL + 4, OUT_D - WALL - 4):
            lid -= Pos(x, y, z - 1) * Cylinder(1.7, LID_H + 2, align=CTR_XY)
    return tag(lid, "Lid (printed)", "#9aa0aa")


# --- Run ---------------------------------------------------------------------

def solids_of(shape):
    return list(shape.solids()) if hasattr(shape, "solids") else [shape]


def main():
    t0 = time.time()
    LIVE.mkdir(exist_ok=True)
    OUT.mkdir(exist_ok=True)
    groups = {
        "base": [base_model()],
        "lid": [lid_model()],
        "pi": [pi_model()],
        "dac": dac_model(),
        "screen": screen_model(),
        "ribbon": ribbon_model(),
    }
    manifest = []
    for name, shapes in groups.items():
        comp = Compound(children=shapes, label=name)
        export_gltf(comp, str(LIVE / f"{name}.glb"), binary=True, linear_deflection=0.1, angular_deflection=0.4)
        manifest.append({"name": name, "file": f"{name}.glb", "printed": name in ("base", "lid")})
    export_stl(groups["base"][0], str(OUT / "base.stl"))
    export_stl(Pos(0, 0, -BASE_H) * groups["lid"][0], str(OUT / "lid.stl"))
    export_step(Compound(children=groups["base"] + groups["lid"]), str(OUT / "enclosure.step"))

    # Collisions: every pair of parts from different groups (a group's own
    # pieces touch by design), by volume of overlap.
    pieces = [(g, s) for g, shapes in groups.items() for s in shapes]
    hits, overlaps = [], []
    bbs = [(g, s, s.bounding_box()) for g, s in pieces]
    for (ga, a, ba), (gb, b, bb) in combinations(bbs, 2):
        if ga == gb or not ba.overlaps(bb, tolerance=0.0):
            continue
        try:
            common = a.intersect(b)
        except Exception as e:  # noqa: BLE001 - a bad boolean is worth reporting, not fatal
            hits.append({"a": a.label, "b": b.label, "error": str(e)})
            continue
        found = common if isinstance(common, list) else ([common] if common else [])
        solids = [x for c in found for x in solids_of(c)]
        vol = sum(x.volume for x in solids)
        if vol > 0.05:
            hits.append({"a": a.label, "b": b.label, "mm3": round(vol, 2)})
            overlaps.append(tag(Compound(solids), f"{a.label} × {b.label}", "#ff2d2d"))
    if overlaps:
        export_gltf(Compound(children=overlaps), str(LIVE / "collisions.glb"), binary=True)
    elif (LIVE / "collisions.glb").exists():
        os.remove(LIVE / "collisions.glb")
    report = {
        "size_mm": [round(OUT_W, 1), round(OUT_D, 1), round(BASE_H + LID_H, 1)],
        "collisions": hits,
        "seconds": round(time.time() - t0, 1),
    }
    (LIVE / "parts.json").write_text(json.dumps(manifest))
    (LIVE / "report.json").write_text(json.dumps(report, indent=1))
    (LIVE / "stamp.txt").write_text(str(time.time_ns()))
    print(json.dumps(report))


if __name__ == "__main__":
    main()
