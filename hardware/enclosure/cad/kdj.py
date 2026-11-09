"""kdj enclosure, slab layout, in build123d: the real parts in a printed box.

The Raspberry Pi (a 4B, or a 5 with PI=5: one base takes either, each has
its own I/O plates) is its published STEP model (parts/, fetched by
cad/fetch-parts.sh); the 5" DSI screen is modelled from its manual (V1.4,
section 3); the InnoMaker HiFi DAC HAT from its listing (its jack positions
are a guess until measured: search MEASURE). The 15-pin ribbon runs from the
Pi's DISPLAY connector to the screen's.

Run in the kdj-cad container (tools/enclosure-live.sh does, on every save):
writes live/*.glb (coloured, for viewer.html), live/parts.json, live/report.json
(sizes, every pair of parts that collide, with the overlap exported as
live/collisions.glb, and every break of the design rules in checks.py), and
out/*.stl / out/*.step for printing. With --check, exits 1 if there are any
(tools/enclosure-check).

Coordinates: mm, Z up, the box's front-left-bottom corner at the origin; the
screen faces up, flush with the top of the walls; the back wall is at max Y.
The prototype sits on a printed stand at ANGLE degrees (default 30). KICKSTAND=1
builds the folding-leg version instead, shown on its TILT=15|30|45 leg.
"""

import json
import math
import os
import sys
import time
import uuid
from itertools import combinations
from pathlib import Path

from build123d import (
    Align,
    Box,
    Circle,
    Color,
    Compound,
    Cylinder,
    Location,
    Plane,
    Polygon,
    Pos,
    RectangleRounded,
    RegularPolygon,
    Rot,
    Shape,
    Shell,
    Solid,
    export_brep,
    export_gltf,
    export_step,
    export_stl,
    extrude,
    import_brep,
    import_step,
    Text,
)

import checks

HERE = Path(__file__).resolve().parent.parent
LIVE = Path(os.environ.get("KDJ_LIVE", HERE / "live"))   # the viewer's; a second variant's check writes elsewhere
OUT = HERE / "out"
PARTS = HERE / "parts"
MIN = (Align.MIN, Align.MIN, Align.MIN)
CTR_XY = (Align.CENTER, Align.CENTER, Align.MIN)

# Hole sizes this printer was measured to make (hardware.toml): every small
# round hole uses one of these, and checks.py fails any that doesn't.
FIT = {name: f["d"] for name, f in checks.load_hardware()[1].items()}
M25 = FIT["M2.5 clearance"]

# --- The screen: 5" DSI display, from its manual --------------------------
SCREEN_W, SCREEN_D = 121.11, 77.93          # module (PCB) outline
LENS_W, LENS_D = 120.5, 75.65               # cover glass
VA_W, VA_D = 109.20, 66.05                  # visible window in the glass print
VA_RIGHT, VA_TOP = 5.65, 2.70               # window from the glass's right / top edge
ACTIVE_W, ACTIVE_D = 108.0, 64.8            # lit pixels
LENS_T = 0.25 + 0.20 + 0.70                 # PET lens + adhesive + touch glass
# Measured (2026-11-08): 7.6 glass to the back of its board, the manual's
# layers taking up 7.25 (the rest put in the gap under the panel); the corner
# nuts end 5.7 from the board's front, 4.1 off its back, and nothing else on
# the back stands taller.
TFT_T, PASTE_T, SPCB_T, NUT_H = 3.0, 1.85, 1.6, 4.1
SCREEN_T = LENS_T + TFT_T + PASTE_T + SPCB_T  # 7.6: glass to the back of its board
CORNER_HOLES = (111.11, 67.93)              # centred: 5 mm in from each edge
# The Pi's holes on the screen (it can mount there): 58 x 49, the left pair
# 38.11 from the left corner hole, the lower pair 16.5 above the bottom one.
SCREEN_PI_HOLES = [(5 + 38.11 + dx, 5 + 16.5 + dy) for dx in (0, 58) for dy in (0, 49)]
FPC_FROM_RIGHT = 13.48                      # ribbon connector from the right edge
FPC_LEN = 46.01                             # it stands on end, centred on this run

# --- The Pi: a 4B or a 5 (PI=4|5) ----------------------------------------------
# The same board outline and mounting holes, so one base takes either; each
# has its own pair of I/O plates, cut round its own ports. The Pi sits where
# the Pi 4's furthest-reaching jacks put it (OVERHANG_*): the Pi 5's reach
# less, so its connector faces sit a little further inside the plates.
PI = os.environ.get("PI", "4")
PI_W, PI_D, PI_T = 85.0, 56.0, 1.6
PI_HOLES = [(3.5, 3.5), (61.5, 3.5), (3.5, 52.5), (61.5, 52.5)]
# The Pi stands on four metal M-F 6+6 standoffs, each stud down through the
# floor into a nut underneath: nothing printed takes the load.
STANDOFF_H, STANDOFF_AF, NUT_T = 6.0, 5.0, 2.0   # body; across flats (standoff and nut); nut thickness
HEATSINK = (14.0, 14.0, 6.0)                 # a stick-on aluminium one; clears the HAT by ~3 mm
OVERHANG_S, OVERHANG_E = 2.50, 3.36          # where the Pi sits: the Pi 4's audio jack and USB 3 flange

# Ports that reach the I/O plates, measured from each STEP, in Pi coordinates
# (x along the 85 mm edge, y from the USB-C edge): name, edge ("s" = USB-C
# edge, "e" = USB-A end), extent along that edge, extent in z above the
# board's bottom, how far it reaches past the edge, shape ("relief": no hole,
# a recess inside the plate so it stays covered), and an opening (w, h) the
# size of a plug's moulded grip, or None. The Pi 4's from every face, solid or
# not; the Pi 5's from each connector's last 1.5 mm before its face.
PIS = {
    "4": {
        "name": "Raspberry Pi 4",
        "step": "raspberry_pi_4_model_b.step", "cache": "pi4-sewn.brep",
        # Its file lies on its side, board in XZ, components toward +Y, its
        # board's top at y = 0 under 0.1 mm copper pads.
        "to_pi": Pos(0, 0, PI_T + 0.1) * Rot(90, 0, 0),
        "ports": [
            ("USB-C", "s", (6.73, 15.67), (0.90, 4.86), 1.20, "rect", (12.5, 7.0)),
            ("HDMI 0", "s", (22.75, 29.25), (1.28, 4.54), 1.20, "relief", None),
            ("HDMI 1", "s", (36.25, 42.75), (1.28, 4.54), 1.20, "relief", None),
            ("Audio", "s", (51.0, 57.0), (1.70, 7.70), 2.50, "round", None),
            ("USB 3", "e", (1.55, 16.45), (0.19, 17.31), 3.36, "rect", None),
            ("USB 2", "e", (19.50, 34.50), (2.39, 18.04), 2.85, "rect", None),
            ("Ethernet", "e", (37.80, 53.70), (-0.20, 15.35), 3.25, "rect", None),
        ],
        "soc": (29.23, 32.63, 4.0),          # centre, and its top above the board's bottom
        "dsi": (2.5, 28.0, 7.5),             # DISPLAY connector (15-pin): centre, top
        "ribbon": (16.0, 7.0 + PI_T),        # its width there; it folds flat this far above the board's bottom
    },
    "5": {
        "name": "Raspberry Pi 5",
        "step": "raspberry_pi_5.step", "cache": "pi5-sewn.brep",
        # Its file lies on its side too, but centred, the board 1.4 thick from y = 0.1.
        "to_pi": Pos(PI_W / 2, PI_D / 2, -0.1) * Rot(90, 0, 0),
        "ports": [
            ("USB-C", "s", (6.83, 15.57), (-0.26, 4.60), 1.30, "rect", (12.5, 7.0)),
            ("HDMI 0", "s", (22.20, 29.40), (1.20, 4.80), 1.66, "relief", None),
            ("HDMI 1", "s", (35.60, 42.80), (1.20, 4.80), 1.66, "relief", None),
            ("Ethernet", "e", (2.20, 18.20), (2.00, 15.30), 3.00, "rect", None),
            ("USB A", "e", (21.75, 36.35), (2.35, 17.85), 3.30, "rect", None),
            ("USB B", "e", (39.70, 54.30), (2.35, 17.85), 3.30, "rect", None),
        ],
        "soc": (33.1, 22.8, 3.6),
        # CAM/DISP 1 (22-pin, 0.5 mm): needs a 22-to-15-pin display cable.
        "dsi": (48.75, 8.45, 5.5),
        "ribbon": (12.0, 11.0),              # the 22-pin end; over the heatsink, under the HAT
    },
}[PI]
PI_NAME, PI_PORTS = PIS["name"], PIS["ports"]
FACE_INSET = 0.3                             # connector faces this far inside the outside surface
HOLE_GAP = 0.3                               # round each connector in its plate

# --- The DAC HAT (InnoMaker HiFi DAC) -------------------------------------
# Measured with calipers, 2026-11-08. It stands on the kit's M-F 15+6
# standoffs (stud down through the Pi into the base's standoffs): 15 mm,
# Pi board top to DAC board underside.
HAT_W, HAT_D, HAT_T = 65.0, 57.0, 1.6
HAT_GAP = 15.0                              # Pi board top to HAT board bottom: the standoffs
HAT_OVER = 0.75                             # its jack edge past the Pi's USB-C edge ("0.5, maybe 1")
JACK_SLACK = 0.5                            # more room above and below each jack than HOLE_GAP: caliper readings (the RCAs are 2.5 apart, no room at the sides)
# Jacks along that edge, as boxes: name, from and to (from its microSD-end
# edge, which is over the Pi's: Pi x), bottom and top above the HAT board's
# underside, reach past its edge. Looking at them from outside: LINE, L
# (white), R (red). All three sit on the board's top.
HAT_JACKS = [("3.5 mm", 12.0, 24.0, HAT_T, 7.0, 3.5),     # top 7 from the underside, about 5 tall
             ("RCA L", 30.0, 41.0, HAT_T, 13.3, 7.0),     # white: 30-41
             ("RCA R", 43.5, 54.5, HAT_T, 13.3, 7.0)]     # red: 44-54, as wide as the white
JACK_DEPTH = 12.0                           # how far each body runs back from the edge (not measured; inside, out of the way)
CUT_JACKS = ("RCA L", "RCA R", "3.5 mm")

# --- The box ---------------------------------------------------------------
WALL, FLOOR, GAP, LIP, CLEARANCE, PORT_GAP = 2.4, 2.0, 0.6, 2.5, 6.7, 1.0
# CLEARANCE: air over the RCAs. 6.7 keeps the box the height it was drawn
# with the DAC 5 mm higher, so the left plate already printed still fits.
IN_W = max(SCREEN_W, PI_W + 10) + 2 * GAP
IN_D = max(SCREEN_D, PI_D + 10) + 2 * GAP
OUT_W, OUT_D = IN_W + 2 * WALL, IN_D + 2 * WALL
BOARD_Z = FLOOR + STANDOFF_H                 # Pi board bottom
STACK_TOP = BOARD_Z + PI_T + HAT_GAP + max(j[4] for j in HAT_JACKS)   # the RCAs are its tallest
BASE_H = STACK_TOP + CLEARANCE + NUT_H       # the screen's back (and nuts) sit on top
PI_X = FACE_INSET + OVERHANG_E               # its USB end's jacks just inside the left face
PI_Y = OUT_D - FACE_INSET - OVERHANG_S - PI_D   # its USB-C edge's jacks just inside the back face
SCREEN_X, SCREEN_Y = (OUT_W - SCREEN_W) / 2, (OUT_D - SCREEN_D) / 2
SCREEN_Z = BASE_H                            # the back of the screen's board
# The screen screws down from below into its own threaded nuts, at its corner
# holes, on posts in the base. Not the back left: the Pi's USB-A jacks are
# under it, so that corner rests on a ledge instead.
SCREEN_CORNERS = [(SCREEN_X + SCREEN_W / 2 + sx * CORNER_HOLES[0] / 2, SCREEN_Y + SCREEN_D / 2 + sy * CORNER_HOLES[1] / 2) for sx in (-1, 1) for sy in (-1, 1)]
POST_TOP = SCREEN_Z - NUT_H
POST_R, BORE_R, SHELF = 5.0, 2.75, 3.0       # bore from below for the screwdriver; M2.5 x 6 through the shelf
TOP_Z = SCREEN_Z + SCREEN_T                  # no lid: the walls rise to the glass, which sits flush

# --- I/O plates ----------------------------------------------------------------
# The back wall, and the left wall where its ports are, are separate plates
# with holes cut close round each connector. Pushed onto the connectors
# first, they go in with the Pi and DAC, sliding down grooves in the base's
# corners: no hole needs room for anything to move through it, so they fit
# close and the connectors sit flush.
BACK_PLATE_X = (14.0, OUT_W - 14.0)
LEFT_PLATE_Y = (20.0, OUT_D - WALL - 0.5)
# Each plate's ends are cut on a slant, matching the slant of the opening's
# ends: the opening is TAPER narrower at the outside face on each side, so a
# plate can't come out, and the Pi's board stops it going in. Lowered in from
# above with the Pi, nothing to slide down, nothing thin.
TAPER, SLIDE = 1.2, 0.15                     # mm the opening narrows to the outside, each end; clearance at the ends
PLATE_LIFT = 0.2                             # off the floor: the connectors set their height
STIFF_D, STIFF_H = 3.0, 1.6                  # a lip along each plate's inside bottom edge: an L, not a flat strip

# --- Air -----------------------------------------------------------------------
# In through the floor (slots placed between the folded legs' crossbars),
# out through the right wall, and out up high, just under the screen, where
# warm air would otherwise pool. An optional 30 mm 5 V fan bolts inside the
# right wall, blowing out, level with the gap between the Pi and the DAC; its
# grille is a vent until then.
FAN_SIZE, FAN_T, FAN_HOLES = 30.0, 10.0, 24.0  # frame, thickness, M3 hole spacing
FAN = os.environ.get("FAN", "0") == "1"           # its screw holes, once there's a fan (and M3 screws) to fit
FAN_AT = (35.0, FLOOR + 1 + 30.0 / 2)          # its centre on the right wall: y, z
HIGH_VENTS_Z = (POST_TOP - 0.7, SCREEN_Z - 0.6)  # a row just under the screen's board

# --- Feet and kickstand -------------------------------------------------------
# Nothing hangs below the floor, so the base prints floor-down without
# support. Under it, screwed on: two hinge rails at the back (their spines
# are the back feet) and two printed front feet; the box stands FEET_H high,
# which lets air in through the floor vents and leaves room for the legs.
# Three nested U-shaped legs share the rails' hinge, one per angle: fold out
# the one you want. Each swings until a tooth on its hub lands flat on the
# floor, a little past vertical, so the box's weight holds it there. A single
# leg can't do this: at the shallow angles it would lie nearly flat and fold
# under a tap.
FEET_H, RUBBER_T, FOOT_R = 8.0, 0.0, 6.0     # RUBBER_T: stick-on pads under the feet and spines, if you add them
FRONT_FEET = [(18.0, FOOT_R + 2), (OUT_W - 18.0, FOOT_R + 2)]   # clear of the screen's screw bores
LEG_T, ARM_W, ARM_GAP, LUG_W, FOOT_W = 6.0, 4.5, 0.5, 3.0, 8.0
AXIS_Y, AXIS_Z = OUT_D - 14.0, -(LEG_T / 2 + 0.6)   # the hinge: a filament pin each side
TOOTH_R = 5.5
SPINE_Y = AXIS_Y + TOOTH_R + 1.0             # the rails' spines start behind the teeth
OVER = 5.0                                   # degrees past vertical
LEGS = (45, 30, 15)                          # outermost (longest) first
SET_W = 2 * LUG_W + ARM_GAP + len(LEGS) * (ARM_W + ARM_GAP)
SETS_X = (3.0, 112.5)                        # left set starts, right set ends: clear of the Pi's and screen's screws
BOSS_H = 2.0                                 # floor bosses the rails' M2.5 x 8 screws thread into
RAIL_SCREWS = [(x, OUT_D - WALL - 2.4) for x0 in (SETS_X[0], SETS_X[1] - SET_W) for x in (x0 + 5.5, x0 + SET_W - 5.5)]
TILT = float(os.environ.get("TILT", 30))     # shown on this leg (0: flat, all folded)

# The whole build needs only M2.5 screws (and the filament it's printed in):
# the hinge pin is a length of 1.75 mm filament, its ends melted over so it
# stays; the rails' M2.5 screws thread straight into the plastic. KICKSTAND=0
# swaps the legs for a simple printed stand at ANGLE degrees.
KICKSTAND = os.environ.get("KICKSTAND", "0") == "1"
PIN_D_LUG, PIN_D_ARM = 1.8, 2.05             # snug in the rails' lugs, free in the legs
ANGLE = float(os.environ.get("ANGLE", 30))

# --- Labels --------------------------------------------------------------------
# Debossed into the outside faces. On their own they read in raking light;
# filled (a second filament, or paint wiped into them) they read at a glance.
# Light on dark, words and not colours, for anyone colourblind.
FONT = str(Path(__file__).resolve().parent / "fonts" / "GeistMono-Bold.ttf")   # Geist Mono (SIL OFL), as in the logo
DEBOSS = 0.6
JACK_NAMES = {"RCA L": "L", "RCA R": "R", "3.5 mm": "LINE"}
REVISION = "KDJ-1  rev A"
# Each face a label can sit on: (the text's x, the face's outward normal), so
# it reads the right way round from outside.
FACES = {"back": ((-1, 0, 0), (0, 1, 0)), "front": ((1, 0, 0), (0, -1, 0)),
         "left": ((0, -1, 0), (-1, 0, 0)), "bottom": ((-1, 0, 0), (0, 0, -1))}


def placed(x, y, z=0.0):
    """A point in the Pi's coordinates, in the box's: the Pi lies turned
    180 degrees, its USB-C edge to the back wall, its USB end to the left."""
    return (PI_X + PI_W - x, PI_Y + PI_D - y, z)


def arm_x(i):
    """x ranges of leg i's two arms (0 = the outermost leg)."""
    off = LUG_W + ARM_GAP + i * (ARM_W + ARM_GAP)
    left = SETS_X[0] + off
    right = SETS_X[1] - off - ARM_W
    return (left, left + ARM_W), (right, right + ARM_W)


def lug_x():
    inner = LUG_W + ARM_GAP + len(LEGS) * (ARM_W + ARM_GAP)
    return [(SETS_X[0], SETS_X[0] + LUG_W), (SETS_X[0] + inner, SETS_X[0] + inner + LUG_W),
            (SETS_X[1] - LUG_W, SETS_X[1]), (SETS_X[1] - inner - LUG_W, SETS_X[1] - inner)]


def opening(angle):
    """How far a leg swings open from folded, for a tilt of angle."""
    return 90 - angle + OVER


def leg_length(angle):
    """Hinge to foot, so the foot meets the desk with the box resting on its
    front feet at angle."""
    t = math.radians(angle)
    py, pz = FRONT_FEET[0][1] - FOOT_R, -FEET_H   # the front feet's front edge: the box pivots on it
    hy, hz = AXIS_Y - py, AXIS_Z - pz
    to_centre = (hy * math.sin(t) + hz * math.cos(t)) / math.sin(math.radians(opening(angle) + angle))
    return to_centre - LEG_T / 2 * math.sin(math.radians(OVER))   # the foot's corner, tipped past vertical, is lowest


def about_axis(deg):
    return Pos(0, AXIS_Y, AXIS_Z) * Rot(deg, 0, 0) * Pos(0, -AXIS_Y, -AXIS_Z)


def on_face(sketch, face, at, depth, proud=0.0):
    """A flat sketch centred at a point on a face, reaching depth into it (and
    proud of it, for a clean cut): cut from a part, or kept as a fill."""
    x_dir, normal = FACES[face]
    solid = Pos(0, 0, -depth) * extrude(sketch, amount=depth + proud)
    return Plane(origin=at, x_dir=x_dir, z_dir=normal).location * solid


def label_solid(text, size, face, at, depth=DEBOSS):
    """Letters size mm tall."""
    return on_face(Text(text, font_size=size, font_path=FONT, align=(Align.CENTER, Align.CENTER)), face, at, depth)


def logo_sketch(h=12.0):
    """The KDJ-1 model plate, h mm tall: a rounded frame, a rivet at each end,
    the name in Geist Mono. Drawn at 320 x 110, strokes kept to 1 mm or more
    so it prints."""
    k = h / 110
    w, r = 320 * k, 16 * k
    stroke = max(8 * k, 1.0)
    frame = RectangleRounded(w, h, r) - RectangleRounded(w - 2 * stroke, h - 2 * stroke, r - stroke)
    rivets = [Pos(sx * 130 * k, 0) * Circle(max(7 * k, 0.9)) for sx in (-1, 1)]
    name = Text("KDJ-1", font_size=58 * k, font_path=FONT, align=(Align.CENTER, Align.CENTER))
    return frame + rivets[0] + rivets[1] + name


def tag(shape, label, color):
    shape.label = label
    shape.color = Color(color)
    return shape


# --- Parts ------------------------------------------------------------------

def sewn(model):
    """The STEP models most parts as loose faces, not solids, and a boolean
    can't see a face. Sew them; those that close up (the HDMI ports among
    them) become solids. The rest stay faces: the distance check sees them."""
    from OCP.BRep import BRep_Tool
    from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeSolid, BRepBuilderAPI_Sewing
    from OCP.TopAbs import TopAbs_SHELL
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopoDS import TopoDS
    in_solids = {hash(sh) for so in model.solids() for sh in so.shells()}
    loose = [sh for sh in model.shells() if hash(sh) not in in_solids]
    sewing = BRepBuilderAPI_Sewing(0.02)
    for sh in loose:
        sewing.Add(sh.wrapped)
    sewing.Perform()
    made, found = [], TopExp_Explorer(sewing.SewedShape(), TopAbs_SHELL)
    while found.More():
        shell = TopoDS.Shell(found.Current())
        if BRep_Tool.IsClosed_s(shell):
            solid = Solid(BRepBuilderAPI_MakeSolid(shell).Solid())
            made.append(solid if solid.volume > 0 else Solid(solid.wrapped.Reversed()))
        found.Next()
    return Compound(list(model.solids()) + made + loose)


def pi_model():
    """The Pi's STEP, upright, its board's USB-C-edge corner at the origin and
    its bottom at z = 0, then turned and placed in the box. Cached as BREP."""
    cache = PARTS / PIS["cache"]
    src = PARTS / PIS["step"]
    if cache.exists() and cache.stat().st_mtime > src.stat().st_mtime:
        pi = import_brep(str(cache))
    else:
        pi = sewn(import_step(str(src)))
        export_brep(pi, str(cache))
    pi = PIS["to_pi"] * pi
    pi = Pos(*placed(0, 0, BOARD_Z)) * Rot(0, 0, 180) * pi
    return tag(pi, PI_NAME, "#2e7d32")


def heatsink_model():
    sx, sy, top = PIS["soc"]
    x, y, _ = placed(sx, sy)
    z = BOARD_Z + top
    return tag(Pos(x, y, z) * Box(*HEATSINK, align=CTR_XY), "Heatsink", "#b0b4ba")


def standoffs_model():
    """The Pi's four M-F 6+6 standoffs, studs down through the floor, and
    their nuts underneath (bought, not printed)."""
    out = []
    for hx, hy in PI_HOLES:
        x, y, _ = placed(hx, hy)
        hex_ = RegularPolygon(STANDOFF_AF / 2, 6, major_radius=False)
        body = Pos(x, y, FLOOR) * extrude(hex_, amount=STANDOFF_H)
        body -= Pos(x, y, FLOOR + 1) * Cylinder(1.25, STANDOFF_H, align=CTR_XY)
        body += Pos(x, y, FLOOR - 6.0) * Cylinder(1.25, 6.0, align=CTR_XY)       # the stud
        out.append(tag(body, "Standoff", "#c9a646"))
        nut = Pos(x, y, -NUT_T) * extrude(hex_, amount=NUT_T) - Pos(x, y, -NUT_T - 1) * Cylinder(1.25, NUT_T + 2, align=CTR_XY)
        out.append(tag(nut, "Nut", "#9aa0a8"))
    return out


def dac_model():
    hz = BOARD_Z + PI_T + HAT_GAP
    parts = []
    # 1 mm deeper than the Pi: HAT_OVER of it past the jacks' edge.
    edge = PI_Y + PI_D + HAT_OVER
    board = Pos(PI_X + PI_W - HAT_W, edge - HAT_D, hz) * Box(HAT_W, HAT_D, HAT_T, align=MIN)
    for hx, hy in PI_HOLES:
        x, y, _ = placed(hx, hy)
        board -= Pos(x, y, hz - 1) * Cylinder(1.35, HAT_T + 2, align=CTR_XY)
    parts.append(tag(board, "DAC board", "#1b5e20"))
    # Stacking socket on the Pi's header (it ends above the header's pins).
    sx, sy, _ = placed(7.1 + 50.8, PI_D - 3.5 + 2.5)
    parts.append(tag(Pos(sx, sy, PI_T + BOARD_Z + 6.0) * Box(50.8, 5.0, hz - PI_T - BOARD_Z - 6.0, align=MIN), "DAC socket", "#111111"))
    cx, cy, _ = placed(30, 28)
    parts.append(tag(Pos(cx, cy, hz + HAT_T) * Box(9.7, 6.4, 1.2, align=CTR_XY), "PCM5122", "#222222"))
    for name, a0, a1, z0, z1, reach in HAT_JACKS:
        x = placed((a0 + a1) / 2, 0)[0]
        body = Pos(x, edge - JACK_DEPTH, hz + z0) * Box(a1 - a0, JACK_DEPTH + reach, z1 - z0, align=(Align.CENTER, Align.MIN, Align.MIN))
        parts.append(tag(body, name, "#f2f2f2" if name == "RCA L" else "#c62828" if name == "RCA R" else "#151515"))
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
    parts.append(tag(Pos(x0 + SCREEN_W - 9, y0 + 14, z - 3.5) * Box(5.0, 2.5, 3.5, align=MIN), "Fan header", "#202020"))
    return parts


def ribbon_model():
    """The display ribbon, as flat runs. The Pi's connector sits under the HAT,
    so the ribbon folds flat just above it, jogs along y if it has to (the Pi
    5's sits near the back, beyond the screen connector's run), runs out under
    the HAT to the screen's connector (a few mm beyond the Pi's edge), and
    rises beside the stack to it, at its 16 mm end."""
    w, t = 16.0, 0.15
    dx, dy, top = PIS["dsi"]
    wp, fold_at = PIS["ribbon"]
    px, py, _ = placed(dx, dy)
    leave, fold = BOARD_Z + top, BOARD_Z + fold_at
    fx = SCREEN_X + SCREEN_W - FPC_FROM_RIGHT
    reach = (FPC_LEN - w) / 2                          # how far either way of centre it can land
    sy = min(max(py, SCREEN_Y + SCREEN_D / 2 - reach), SCREEN_Y + SCREEN_D / 2 + reach)
    runs = [
        Pos(px - t / 2, py - wp / 2, leave) * Box(t, wp, fold - leave, align=MIN),
        Pos(min(px, fx), sy - wp / 2, fold) * Box(abs(fx - px), wp, t, align=MIN),
        Pos(fx - t / 2, sy - w / 2, fold) * Box(t, w, SCREEN_Z - 2.5 - fold, align=MIN),
    ]
    if abs(sy - py) > 0.01:                            # the jog, flat, beside the Pi's connector
        runs.append(Pos(px - wp / 2, min(py, sy), fold) * Box(wp, abs(sy - py), t, align=MIN))
    return [tag(r, "Ribbon", "#e0e0e0") for r in runs]


# --- The printed box ---------------------------------------------------------

def plate_holes():
    """(face, cutter) for every hole in the plates: each connector's outline
    plus HOLE_GAP, a pocket outside where a plug's grip reaches in, and a
    window over the screen's backlight key."""
    out = []
    for name, edge, (a0, a1), (z0, z1), reach, shape, pocket in PI_PORTS:
        face, u, z = ("back" if edge == "s" else "left"), (a0 + a1) / 2, BOARD_Z + (z0 + z1) / 2
        at = (placed(u, 0)[0], OUT_D, z) if edge == "s" else (0, placed(0, u)[1], z)
        w, h = a1 - a0 + 2 * HOLE_GAP, z1 - z0 + 2 * HOLE_GAP
        into = reach - (FACE_INSET + (OVERHANG_S if edge == "s" else OVERHANG_E) - WALL)   # how far it reaches into the plate
        if shape == "relief":                       # covered: a recess on the inside face
            x = placed(u, 0)[0]
            out.append((face, Pos(x - w / 2, OUT_D - WALL - 0.5, z - h / 2) * Box(w, 0.5 + into + HOLE_GAP, h, align=MIN)))
            continue
        # A plug whose moulded grip has to reach in (USB-C) gets an opening
        # that size, straight through: a pocket would leave a thin frame.
        outline = RectangleRounded(*pocket, 1.5) if pocket else Circle(w / 2) if shape == "round" else RectangleRounded(w, h, 0.6)
        out.append((face, on_face(outline, face, at, WALL + 1, proud=0.5)))
        # A hole closer to the plate's end than a wall can be thin runs out
        # through the end instead: the base's corner closes it.
        ow = pocket[0] if pocket else w
        ends = BACK_PLATE_X if edge == "s" else LEFT_PLATE_Y
        c = at[0] if edge == "s" else at[1]
        for end, sign in ((ends[0] + SLIDE, -1), (ends[1] - SLIDE, 1)):
            if sign * (c + sign * ow / 2 - (end - sign * TAPER)) > -checks.MIN_WALL:   # to the end at the outside face, its nearest
                near, far = c, end + sign * 2
                lo, span = min(near, far), abs(far - near)
                hh = pocket[1] if pocket else h
                cut = (Pos(lo, OUT_D - WALL - 1, z - hh / 2) * Box(span, WALL + 2, hh, align=MIN) if edge == "s"
                       else Pos(-1, lo, z - hh / 2) * Box(WALL + 2, span, hh, align=MIN))
                out.append((face, cut))
    hz = BOARD_Z + PI_T + HAT_GAP + HAT_T
    for name, a0, a1, z0, z1, _ in HAT_JACKS:       # each jack's body, HOLE_GAP round
        if name in CUT_JACKS:
            at = (placed((a0 + a1) / 2, 0)[0], OUT_D, hz - HAT_T + (z0 + z1) / 2)
            out.append(("back", on_face(RectangleRounded(a1 - a0 + 2 * HOLE_GAP, z1 - z0 + 2 * (HOLE_GAP + JACK_SLACK), 0.6), "back", at, WALL + 1, proud=0.5)))
    # The DAC's board edge reaches HAT_OVER past the Pi's, into the plate: a
    # groove inside for it.
    into = HAT_OVER - (FACE_INSET + OVERHANG_S - WALL)
    if into > -HOLE_GAP:
        out.append(("back", Pos(PI_X + PI_W - HAT_W - HOLE_GAP, OUT_D - WALL - 0.5, hz - HAT_T - JACK_SLACK - HOLE_GAP)
                    * Box(HAT_W + 2 * HOLE_GAP, 0.5 + into + HOLE_GAP, HAT_T + 2 * (JACK_SLACK + HOLE_GAP), align=MIN)))
    out.append(("back", Pos(SCREEN_X + 5 + 15.03 - 1, OUT_D - WALL - 1, BASE_H - 5) * Box(8, WALL + 2, 6, align=MIN)))
    return out


def plate_labels():
    """(face, letters): port names beside their holes, positioned from them."""
    ports = {p[0]: p for p in PI_PORTS}
    hz = BOARD_Z + PI_T + HAT_GAP + HAT_T
    jacks_top = max(hz - HAT_T + j[4] + HOLE_GAP + JACK_SLACK for j in HAT_JACKS if j[0] in CUT_JACKS)
    out = [("back", JACK_NAMES[n], 4.0, (placed((a0 + a1) / 2, 0)[0], OUT_D, jacks_top + 4)) for n, a0, a1, *_ in HAT_JACKS if n in CUT_JACKS]
    usbc_z = BOARD_Z + sum(ports["USB-C"][3]) / 2
    low = (FLOOR + PLATE_LIFT + usbc_z - ports["USB-C"][6][1] / 2) / 2   # under the USB-C pocket
    out.append(("back", "5V", 3.0, (placed(sum(ports["USB-C"][2]) / 2, 0)[0], OUT_D, low)))
    if "Audio" in ports:                            # the Pi 5 has none
        out.append(("back", "AUX", 3.0, (placed(sum(ports["Audio"][2]) / 2, 0)[0], OUT_D, low)))
    side_z = BOARD_Z + max(p[3][1] for p in PI_PORTS if p[1] == "e") + HOLE_GAP + 4
    usbs = [p for p in PI_PORTS if p[1] == "e" and p[0].startswith("USB")]
    usb_y = sum(placed(0, sum(p[2]) / 2)[1] for p in usbs) / len(usbs)
    out.append(("left", "USB", 4.0, (0, usb_y, side_z)))
    out.append(("left", "LAN", 4.0, (0, placed(0, sum(ports["Ethernet"][2]) / 2)[1], side_z)))
    return [(face, label_solid(t, size, face, at)) for face, t, size, at in out]


def base_labels():
    """The KDJ-1 plate on the front, a revision underneath."""
    return [on_face(logo_sketch(), "front", (OUT_W / 2, 0, TOP_Z / 2), DEBOSS),
            label_solid(REVISION, 4.0, "bottom", (OUT_W / 2, FRONT_FEET[0][1] + 3, 0), depth=0.4)]


def slanted(face, clear=0.0, z0=FLOOR, z1=TOP_Z + 1):
    """The prism a plate fills in its wall: across the opening, its ends
    slanting TAPER in towards the outside face. clear shrinks it at the ends
    (the plate); the base's cut is the full size, overshooting both faces."""
    over = 1.0
    if face == "back":
        (a0, a1), inner, outer = BACK_PLATE_X, OUT_D - WALL, OUT_D
    else:
        (a0, a1), inner, outer = LEFT_PLATE_Y, WALL, 0.0
    def at(v):                                      # the ends' positions at depth v through the wall
        k = (v - inner) / (outer - inner) * TAPER
        return a0 + clear + k, a1 - clear - k
    d = 1 if outer > inner else -1
    v_in, v_out = (inner, outer) if clear else (inner - d * over, outer + d * over)
    pts = [(at(v_in)[0], v_in), (at(v_in)[1], v_in), (at(v_out)[1], v_out), (at(v_out)[0], v_out)]
    if face == "left":
        pts = [(v, u) for u, v in pts]
    return Pos(0, 0, z0) * extrude(Polygon(*pts, align=None), amount=z1 - z0, dir=(0, 0, 1))


def stiffener(face):
    """The lip along a plate's inside bottom edge, notched round the rails'
    bosses and the Pi's posts."""
    z = FLOOR + PLATE_LIFT
    if face == "back":
        x0, x1 = BACK_PLATE_X
        lip = Pos(x0 + SLIDE + 0.5, OUT_D - WALL - STIFF_D, z) * Box(x1 - x0 - 2 * SLIDE - 1, STIFF_D + 0.01, STIFF_H, align=MIN)
    else:
        y0, y1 = LEFT_PLATE_Y
        lip = Pos(WALL - 0.01, y0 + SLIDE + 0.5, z) * Box(STIFF_D + 0.01, y1 - y0 - 2 * SLIDE - 1, STIFF_H, align=MIN)
    for x, y in RAIL_SCREWS if KICKSTAND else []:
        lip -= Pos(x, y, 0) * Cylinder(3.8, TOP_Z, align=CTR_XY)
    for hx, hy in PI_HOLES:
        x, y, _ = placed(hx, hy)
        lip -= Pos(x, y, 0) * Cylinder(3.3, TOP_Z, align=CTR_XY)
    return lip


def back_plate(lettered=True):
    """Prints outside face down."""
    x0, x1 = BACK_PLATE_X
    plate = slanted("back", SLIDE, FLOOR + PLATE_LIFT, TOP_Z)
    plate += stiffener("back")
    for face, cut in plate_holes() + (plate_labels() if lettered else []):
        if face == "back":
            plate -= cut
    return tag(plate, "Back plate (printed)", "#7c8492")


def left_plate(lettered=True):
    """With the shelf for the screen's back-left corner, which sits over the
    USB jacks and so has to come in with them. Prints outside face down, the
    shelf standing up."""
    y0, y1 = LEFT_PLATE_Y
    plate = slanted("left", SLIDE, FLOOR + PLATE_LIFT, TOP_Z)
    plate += stiffener("left")
    x, y = min(SCREEN_CORNERS, key=lambda p: p[0] - p[1])
    shelf_z = BOARD_Z + max(p[3][1] for p in PI_PORTS if p[1] == "e") + 0.6
    plate += Pos(WALL - 0.01, y - POST_R, shelf_z) * Box(x + POST_R - WALL + 0.01, y1 - SLIDE - (y - POST_R), POST_TOP - shelf_z, align=MIN)
    for face, cut in plate_holes() + (plate_labels() if lettered else []):
        if face == "left":
            plate -= cut
    return tag(plate, "Left plate (printed)", "#7c8492")


def leg_label(i, angle):
    """Its angle, on the underside of its foot: readable with the legs folded."""
    (l0, _), (_, r1) = arm_x(i)
    return label_solid(str(angle), 5.0, "bottom", ((l0 + r1) / 2, AXIS_Y - leg_length(angle) + FOOT_W / 2, AXIS_Z - LEG_T / 2), depth=0.5)


def vents():
    """Cutters for the air: the floor slots, the fan's grille and holes, and
    the high rows on the right and front walls."""
    cuts = []
    # Floor: across the Pi's footprint, clear of the legs' arms, in pieces
    # between where their crossbars lie folded.
    bands = [(AXIS_Y - leg_length(a) - 1, AXIS_Y - leg_length(a) + FOOT_W + 1) for a in LEGS]
    for x in [23.0 + 6 * i for i in range(12)]:
        y, end = PI_Y + 9, PI_Y + 47
        for b0, b1 in sorted(bands) + [(end, end)]:
            if b1 <= y:
                continue
            if min(b0, end) - y >= 4:
                cuts.append(Pos(x, y, -1) * Box(3, min(b0, end) - y, FLOOR + 2, align=MIN))
            y = max(y, b1)
    # Right wall: the fan's grille (upright slots inside its circle) and its
    # four M3 holes; then the high row.
    fy, fz = FAN_AT
    r = FAN_SIZE / 2 - 1.5
    for dy in [-12.6 + 3.6 * i for i in range(8)]:   # 2 mm slots, 1.6 mm bars
        h = 2 * math.sqrt(max(r * r - (dy + 1) ** 2, 0)) - 1
        if h > 3:
            cuts.append(Pos(OUT_W - WALL - 1, fy + dy, fz - h / 2) * Box(WALL + 2, 2, h, align=MIN))
    for sy in (-1, 1) if FAN else ():
        for sz in (-1, 1):
            cuts.append(Pos(OUT_W - WALL - 1, fy + sy * FAN_HOLES / 2, fz + sz * FAN_HOLES / 2) * Rot(0, 90, 0) * Cylinder(1.7, WALL + 2, align=CTR_XY))
    z0, z1 = HIGH_VENTS_Z
    for y in [14.0 + 5 * i for i in range(12)]:
        cuts.append(Pos(OUT_W - WALL - 1, y, z0) * Box(WALL + 2, 2.5, z1 - z0, align=MIN))
    for x in [22.0 + 5 * i for i in range(17)]:
        cuts.append(Pos(x, -1, z0) * Box(2.5, WALL + 2, z1 - z0, align=MIN))
    return cuts


def fan_model():
    """The optional fan, for the collision check (bought, not printed)."""
    fy, fz = FAN_AT
    fan = Pos(OUT_W - WALL - FAN_T, fy - FAN_SIZE / 2, fz - FAN_SIZE / 2) * Box(FAN_T, FAN_SIZE, FAN_SIZE, align=MIN)
    fan -= Pos(OUT_W - WALL - FAN_T - 1, fy, fz) * Rot(0, 90, 0) * (Cylinder(FAN_SIZE / 2 - 1.5, FAN_T + 2, align=CTR_XY) - Cylinder(6, FAN_T + 2, align=CTR_XY))
    return tag(fan, "Fan (30 mm, optional)", "#3d4450")


def base_model(lettered=True):
    """Printed floor-down with no support: nothing hangs below the floor, and
    nothing inside overhangs past 45 degrees."""
    base = Box(OUT_W, OUT_D, TOP_Z, align=MIN) - Pos(WALL, WALL, FLOOR) * Box(IN_W, IN_D, TOP_Z, align=MIN)
    (x0, x1), (y0, y1) = BACK_PLATE_X, LEFT_PLATE_Y   # openings for the I/O plates, and their grooves
    base -= slanted("back")
    base -= slanted("left")
    for cut in vents():
        base -= cut
    for hx, hy in PI_HOLES:                         # the standoffs' studs, down through the flat floor
        x, y, _ = placed(hx, hy)
        base -= Pos(x, y, -1) * Cylinder(M25 / 2, FLOOR + 2, align=CTR_XY)
    back_left = min(SCREEN_CORNERS, key=lambda p: p[0] - p[1])
    for x, y in SCREEN_CORNERS:
        if (x, y) == back_left:                     # its shelf is on the left plate
            continue
        base += Pos(x, y, 0) * Cylinder(POST_R, POST_TOP, align=CTR_XY)
        base -= Pos(x, y, -1) * Cylinder(BORE_R, POST_TOP - SHELF + 1, align=CTR_XY)
        base -= Pos(x, y, -1) * Cylinder(M25 / 2, POST_TOP + 2, align=CTR_XY)
    for x, y in RAIL_SCREWS if KICKSTAND else []:  # bosses for the rails' inserts, pressed in from below, short of the back plate
        base += (Pos(x, y, FLOOR) * Cylinder(3.5, BOSS_H, align=CTR_XY)) & (Pos(0, 0, 0) * Box(OUT_W, OUT_D - WALL - SLIDE, TOP_Z, align=MIN))
        base -= Pos(x, y, -1) * Cylinder(1.1, FLOOR + BOSS_H + 2, align=CTR_XY)   # 2.2 mm: M2.5 cuts its own thread
    for x, y in FRONT_FEET if KICKSTAND else []:    # shallow recesses to locate the front feet
        base -= Pos(x, y, -1) * Cylinder(FOOT_R + 0.2, 1.6, align=CTR_XY)
    for letters in base_labels() if lettered else []:
        base -= letters
    return tag(base, "Base (printed)", "#6b7380")


def rail_model(side):
    """A hinge rail, screwed under the back of the floor: lugs for the pin, and a
    spine behind the legs' teeth that is also a back foot. Prints spine-face
    down."""
    x0 = SETS_X[0] if side == 0 else SETS_X[1] - SET_W
    bottom = -(FEET_H - RUBBER_T)
    rail = Pos(x0, SPINE_Y, bottom) * Box(SET_W, OUT_D - 0.5 - SPINE_Y, -bottom, align=MIN)
    for l0, l1 in lug_x()[2 * side:2 * side + 2]:
        rail += Pos(l0, AXIS_Y - 4.5, AXIS_Z - 3) * Box(l1 - l0, SPINE_Y - AXIS_Y + 4.5 + 0.01, 3 - AXIS_Z, align=MIN)
    rail -= Pos(0, AXIS_Y, AXIS_Z) * Rot(0, 90, 0) * Cylinder(PIN_D_LUG / 2, 3 * OUT_W)
    for x, y in RAIL_SCREWS[2 * side:2 * side + 2]:  # M2.5 x 8 up into the floor's bosses, heads recessed
        rail -= Pos(x, y, bottom - 1) * Cylinder(1.4, -bottom + 2, align=CTR_XY)
        rail -= Pos(x, y, bottom - 1) * Cylinder(2.6, 1 + (-bottom - 4.0), align=CTR_XY)   # leaves the screw 8 mm to the boss's top
    return tag(rail, f"Hinge rail, {'left' if side == 0 else 'right'}", "#8a6fb3")


def foot_model(x, y):
    """A front foot: glued into its recess, a rubber pad under it."""
    return tag(Pos(x, y, -(FEET_H - RUBBER_T)) * Cylinder(FOOT_R, FEET_H - RUBBER_T + 0.5, align=CTR_XY), "Front foot", "#8a6fb3")


def tooth(angle, a0):
    """The hub's tooth for one arm, folded. Open, its top face lies flat on the
    floor. It's kept 2.5 mm or more behind the hinge, where it rises steeply
    as the leg opens: it meets the floor only at full open, and a 0.1 mm
    printing error moves the stop by about 2 degrees. Folded, it stays inside
    the arm's thickness, so the leg prints flat."""
    a = opening(angle)
    disc = Pos(a0, AXIS_Y, AXIS_Z) * Rot(0, 90, 0) * Cylinder(TOOTH_R, ARM_W, align=(Align.CENTER, Align.CENTER, Align.MIN))
    behind = Pos(a0, AXIS_Y + 2.5, AXIS_Z) * Box(ARM_W, 10, -AXIS_Z, align=MIN)    # above the hinge, up to the floor
    flat_when_folded = about_axis(a) * (Pos(a0, AXIS_Y - 50, AXIS_Z - LEG_T / 2) * Box(ARM_W, 100, LEG_T, align=MIN))
    return about_axis(-a) * (disc & behind & flat_when_folded)


def leg_model(i, angle, open_, lettered=True):
    """A U: two arms from the hinge, joined at the foot, each with a round hub
    and a tooth. Folded unless open_. Prints flat."""
    length = leg_length(angle)
    (l0, l1), (r0, r1) = arm_x(i)
    y0, z0 = AXIS_Y - length, AXIS_Z - LEG_T / 2
    leg = Pos(l0, y0, z0) * Box(r1 - l0, FOOT_W, LEG_T, align=MIN)
    for a0 in (l0, r0):
        leg += Pos(a0, y0, z0) * Box(ARM_W, length, LEG_T, align=MIN)
        leg += Pos(a0, AXIS_Y, AXIS_Z) * Rot(0, 90, 0) * Cylinder(LEG_T / 2, ARM_W, align=(Align.CENTER, Align.CENTER, Align.MIN))
        leg += tooth(angle, a0)
    leg -= Pos(0, AXIS_Y, AXIS_Z) * Rot(0, 90, 0) * Cylinder(PIN_D_ARM / 2, 3 * OUT_W)
    if lettered:
        leg -= leg_label(i, angle)
    if open_:
        leg = about_axis(opening(angle)) * leg
    return tag(leg, f"Leg {angle}°", ["#3b6fb6", "#d08a2e", "#5a9a4a"][i])


# --- Run ---------------------------------------------------------------------

def atomic(path, write):
    """Write to a temporary name, then rename into place, so the viewer (or a
    second build racing this one) never reads half a file."""
    tmp = path.with_name(f"{path.stem}.tmp{uuid.uuid4().hex}{path.suffix}")   # every container's pid is 1
    write(str(tmp))
    os.replace(tmp, path)


def solids_of(shape):
    return list(shape.solids()) if hasattr(shape, "solids") else [shape]


def stand_model():
    """The prototype's stand: a wedge under each side edge of the box with a
    stop at its front, joined by a bar on the desk. The middle stays open, so
    air reaches the floor vents. Built in the box's frame (its floor at z = 0);
    prints as it stands, no support."""
    above_desk = Rot(-ANGLE, 0, 0) * Box(4 * OUT_W, 4 * OUT_D, 200, align=(Align.CENTER, Align.CENTER, Align.MIN))
    stand = None
    for x0 in (3.0, OUT_W - 11.0):
        rib = Pos(x0, -4, -100) * Box(8, OUT_D + 4, 100, align=MIN)          # under the floor
        rib += Pos(x0, -4, -100) * Box(8, 4, 105, align=MIN)                 # the stop, 5 mm up the front
        rib &= above_desk
        stand = rib if stand is None else stand + rib
    depth = OUT_D * math.cos(math.radians(ANGLE))
    stand += Rot(-ANGLE, 0, 0) * (Pos(3, 6, 0) * Box(OUT_W - 6, depth - 8, 2.4, align=MIN))
    return tag(stand, f"Stand, {ANGLE:g}°", "#8a6fb3")


PRINTED = ("base", "plates", "stand", "rails", "feet", "legs")

# Everything bought that goes in, checked against hardware.toml (checks.py):
# a screw passes `through` mm before it threads, then needs enough thread in
# what it's `into`, and no more than the `depth` there.
USES = [
    {"part": "standoff M2.5 M-F 6+6", "count": 4, "where": "Pi posts: stud down through the floor into a nut",
     "through": FLOOR, "length": 6.0, "into": "nut"},
    {"part": "nut M2.5", "count": 4, "where": "under the Pi posts"},
    {"part": "standoff M2.5 M-F 15+6", "count": 4, "where": "Pi to DAC: stud down through the Pi into the base's standoffs",
     "through": PI_T, "length": 6.0, "into": "metal", "depth": STANDOFF_H - 1},
    {"part": "screw M2.5x6", "count": 4, "where": "the DAC, down into its standoffs",
     "through": HAT_T, "into": "metal", "depth": 5.0},
    {"part": "screw M2.5x6", "count": 4, "where": "screen, up through the posts' shelves into its nuts",
     "through": SHELF, "into": "metal", "depth": NUT_H},
] + ([
    {"part": "screw M2.5x8", "count": 4, "where": "hinge rails, up into the floor's bosses",
     "through": 4.0, "into": "plastic", "depth": FLOOR + BOSS_H},
    {"part": "filament 1.75 mm", "count": 2, "where": "hinge pins"},
] if KICKSTAND else [])

# Findings the design means, and why: (check, part, mm or None, why).
ALLOW = [
    ("thin", "Back plate", 1.3 if PI == "4" else 0.84,
     "the cover over the unused micro-HDMI ports: a small flat patch, printed as solid bottom layers, not a wall that takes load"),
]

# Parts that go into each other by design: the Pi's header pins into the DAC's socket.
MATES = [{PI_NAME, "DAC socket"}]


def boxes_meet(a, b):
    """Bounding boxes share some volume. (BoundBox.overlaps is False when one
    holds the other, which is every part inside the base.)"""
    return all(lo1 < hi2 and lo2 < hi1 for lo1, hi1, lo2, hi2 in zip(a.min, a.max, b.min, b.max))


def main():
    t0 = time.time()
    LIVE.mkdir(exist_ok=True)
    OUT.mkdir(exist_ok=True)
    groups = {
        "base": [base_model()],
        "plates": [back_plate(), left_plate()],
    }
    if KICKSTAND:
        groups["legs"] = [leg_model(i, a, a == TILT) for i, a in enumerate(LEGS)]
        groups["rails"] = [rail_model(0), rail_model(1)]
        groups["feet"] = [foot_model(x, y) for x, y in FRONT_FEET]
    else:
        groups["stand"] = [stand_model()]
    groups |= {
        "pi": [pi_model(), heatsink_model()] + standoffs_model(),
        **({"fan": [fan_model()]} if FAN else {}),
        "dac": dac_model(),
        "screen": screen_model(),
        "ribbon": ribbon_model(),
        # The letters' fill, as a second filament or paint would leave it.
        "labels": [tag(Compound(base_labels()), "Label fill", "#f4f5f7")] +
                  [tag(Compound([cut for face, cut in plate_labels() if face == f]), "Label fill", "#f4f5f7") for f in ("back", "left")] +
                  [tag(about_axis(opening(a)) * leg_label(i, a) if a == TILT else leg_label(i, a), "Label fill", "#f4f5f7")
                   for i, a in enumerate(LEGS) if KICKSTAND],
    }
    if KICKSTAND:                                   # on its front feet and the TILT leg
        pivot_y = FRONT_FEET[0][1] - FOOT_R
        on_desk = Pos(0, pivot_y, 0) * Rot(TILT, 0, 0) * Pos(0, -pivot_y, FEET_H)
    else:                                           # on the stand, its front bottom edge on the desk
        on_desk = Rot(ANGLE, 0, 0)
    manifest = []
    for name, shapes in groups.items():
        comp = Compound(children=shapes, label=name)   # untilted: export_gltf misplaces a rotated compound
        atomic(LIVE / f"{name}.glb", lambda f: export_gltf(comp, f, binary=True, linear_deflection=0.1, angular_deflection=0.4))
        manifest.append({"name": name, "file": f"{name}.glb", "printed": name in PRINTED})
    # For printing: the base floor-down, the rails spine-face down, the feet
    # and legs flat.
    atomic(OUT / "base.stl", lambda f: export_stl(groups["base"][0], f))
    # Quick test prints, no DAC measurements needed. The base's back-left
    # side, 10 mm high: the Pi's posts, both plates' grooves, the rails'
    # bosses (screw the rails and a leg on to try the hinge). And a template
    # the size of the screen's opening, holed where its nuts should land.
    # From y 6: cut at 18 it left 2 mm of left wall in front of the plate's
    # opening, a pole the plate's end snapped off.
    corner = groups["base"][0] & (Pos(0, 6, 0) * Box(115, OUT_D - 6, 10, align=MIN))
    atomic(OUT / "test-corner.stl", lambda f: export_stl(corner, f))
    template = Box(SCREEN_W + 2 * GAP, SCREEN_D + 2 * GAP, 1.2, align=MIN)
    nuts = [(SCREEN_W / 2 + sx * CORNER_HOLES[0] / 2, SCREEN_D / 2 + sy * CORNER_HOLES[1] / 2) for sx in (-1, 1) for sy in (-1, 1)]
    for hx, hy in nuts + SCREEN_PI_HOLES:
        template -= Pos(GAP + hx, GAP + hy, -1) * Cylinder(2.5 + 0.4, 4, align=CTR_XY)
    atomic(OUT / "test-screen.stl", lambda f: export_stl(template, f))
    kickstand_files = ["rail-left.stl", "rail-right.stl", "foot.stl"] + [f"leg-{a}{x}.stl" for a in LEGS for x in ("", "-label")]
    if KICKSTAND:
        (OUT / "stand.stl").unlink(missing_ok=True)       # stale, from a KICKSTAND=0 build
        for side, rail in zip(("left", "right"), groups["rails"]):
            atomic(OUT / f"rail-{side}.stl", lambda f: export_stl(Rot(180, 0, 0) * rail, f))
        atomic(OUT / "foot.stl", lambda f: export_stl(Pos(0, 0, FEET_H - RUBBER_T) * groups["feet"][0], f))
    else:
        atomic(OUT / "stand.stl", lambda f: export_stl(on_desk * groups["stand"][0], f))
        for name in kickstand_files:                # stale files from a kickstand build
            (OUT / name).unlink(missing_ok=True)
    # The fills, for a second filament: load with the part they belong to, as
    # a part of the same object, and give them the other filament.
    atomic(OUT / "base-labels.stl", lambda f: export_stl(groups["labels"][0], f))
    # The plates outside face down: the back turned about x, the left about y.
    flat = {"back": Pos(0, 0, OUT_D) * Rot(-90, 0, 0), "left": Rot(0, -90, 0)}
    for (face, plate), fill in zip(zip(("back", "left"), groups["plates"]), groups["labels"][1:3]):
        # Each Pi has its own pair: plate-back-pi4.stl, plate-left-pi5.stl, ...
        atomic(OUT / f"plate-{face}-pi{PI}.stl", lambda f: export_stl(flat[face] * plate, f))
        atomic(OUT / f"plate-{face}-pi{PI}-labels.stl", lambda f: export_stl(flat[face] * fill, f))
        for stale in (f"plate-{face}.stl", f"plate-{face}-labels.stl"):   # from before there were two
            (OUT / stale).unlink(missing_ok=True)
    for i, angle in enumerate(LEGS) if KICKSTAND else []:
        atomic(OUT / f"leg-{angle}-label.stl", lambda f: export_stl(Pos(0, 0, LEG_T / 2 - AXIS_Z) * leg_label(i, angle), f))
        atomic(OUT / f"leg-{angle}.stl", lambda f: export_stl(Pos(0, 0, LEG_T / 2 - AXIS_Z) * leg_model(i, angle, False), f))
    atomic(OUT / "enclosure.step", lambda f: export_step(Compound(children=[s for g in PRINTED if g in groups for s in groups[g]]), f))

    # Collisions: every pair of parts from different groups (a group's own
    # pieces touch by design), by volume of overlap.
    # The lettering is 0.6 mm into 2.4 mm outer walls, so it can't touch
    # anything inside: check an unlettered base, which is far quicker. The
    # fills sit in their own cuts.
    # The Pi's loose faces make booleans crawl and can't overlap anything:
    # the overlap check takes its solids, the distance check below all of it.
    pi_full = groups["pi"][0]
    pi_solids = tag(Compound(list(pi_full.solids())), pi_full.label, "#2e7d32")
    checked = dict(groups, base=[base_model(lettered=False)], plates=[back_plate(lettered=False), left_plate(lettered=False)],
                   pi=[pi_solids] + groups["pi"][1:])
    pieces = [(g, s) for g, shapes in checked.items() if g != "labels" for s in shapes]
    hits, overlaps = [], []
    bbs = [(g, s, s.bounding_box()) for g, s in pieces]
    for (ga, a, ba), (gb, b, bb) in combinations(bbs, 2):
        if ga == gb or {a.label, b.label} in MATES or not boxes_meet(ba, bb):
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
    # Faces the overlap check can't see (most of the Pi's STEP): any printed
    # part but the base (which the Pi sits on) within 0.1 mm of the Pi or DAC.
    for g in ("plates", "stand", "rails", "legs", "feet"):
        for a in checked.get(g, []):
            for b in groups["pi"] + checked["dac"]:
                if boxes_meet(a.bounding_box(), b.bounding_box()) and a.distance_to(b) < 0.1:
                    hits.append({"a": a.label, "b": b.label, "touching": True})
    # Design rules (checks.py), on each printed part as it sits on the bed.
    # Unlettered: the letters are shallow cuts in thick walls.
    to_print = [(checked["base"][0], Location()),
                (checked["plates"][0], flat["back"]), (checked["plates"][1], flat["left"])]
    if KICKSTAND:
        to_print += [(rail, Rot(180, 0, 0)) for rail in groups["rails"]] + [(groups["feet"][0], Location())]
        to_print += [(leg_model(i, a, False, lettered=False), Location()) for i, a in enumerate(LEGS)]
    else:
        to_print += [(groups["stand"][0], on_desk)]
    problems = checks.run([(shape.label, shape, loc) for shape, loc in to_print], USES, ALLOW)
    if overlaps:
        atomic(LIVE / "collisions.glb", lambda f: export_gltf(Compound(children=overlaps), f, binary=True))
    elif (LIVE / "collisions.glb").exists():
        os.remove(LIVE / "collisions.glb")
    report = {
        "size_mm": [round(OUT_W, 1), round(OUT_D, 1), round(TOP_Z + (FEET_H if KICKSTAND else 0), 1)],
        "pi": PI_NAME,
        "angle": TILT if KICKSTAND else ANGLE,
        # The parts are exported in the box's frame; the viewer tilts them onto the desk with this (row-major 4x4).
        "on_desk": [on_desk.wrapped.Transformation().Value(r, c) for r in (1, 2, 3) for c in (1, 2, 3, 4)] + [0, 0, 0, 1],
        "collisions": hits,
        "problems": problems,
        "seconds": round(time.time() - t0, 1),
    }
    atomic(LIVE / "parts.json", lambda f: Path(f).write_text(json.dumps(manifest)))
    atomic(LIVE / "report.json", lambda f: Path(f).write_text(json.dumps(report, indent=1)))
    atomic(LIVE / "stamp.txt", lambda f: Path(f).write_text(str(time.time_ns())))
    for c in hits:
        print(f"✕ collision {c['a']} × {c['b']}: " + ("couldn't check" if "error" in c else "touching" if c.get("touching") else f"{c['mm3']} mm³"))
    for f in problems:
        print(checks.describe(f))
    print(f"{len(hits)} collisions, {len(problems)} problems, {report['seconds']} s")
    return not hits and not problems


if __name__ == "__main__":
    ok = main()
    sys.exit(0 if ok or "--check" not in sys.argv else 1)
