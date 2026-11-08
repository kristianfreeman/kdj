"""Design rules for the printed parts, checked on every build (kdj.py runs
them; tools/enclosure-check fails on them). Each rule is one that a real
print broke:

  thin       Plastic thinner than MIN_WALL between two of a part's surfaces
             that face away from each other: a wall, a web, or a neck where
             two thick pieces join through a thin ring (the Pi's posts did, and
             snapped off).
  hole       A round hole under FASTENER_HOLE across that isn't one of the
             fits measured on the test strip (hardware.toml). Printed holes
             come out small by an amount only a test print tells.
  hardware   A screw, standoff or nut the build needs that isn't on hand, or a
             screw too short to thread far enough into what it's for, or so
             long it bottoms out.
  overhang   A downward face, as the part sits on the bed, steeper than
             OVERHANG degrees from vertical, or a flat ceiling wider than
             BRIDGE: either needs support.

A finding the design means (a tongue thin on purpose) is allowed by name, with
the reason, in the model: see kdj.ALLOW.
"""

import math
import re
import tomllib
from collections import defaultdict
from pathlib import Path

import numpy as np
from build123d import Location, Vector

MIN_WALL = 1.6          # four perimeters of a 0.4 mm nozzle
FASTENER_HOLE = 5.0     # round holes smaller than this are for screws, pins, standoffs
OVERHANG = 45.0         # degrees from vertical
BRIDGE = 10.0           # mm, the widest flat ceiling printed with nothing under it
MIN_THREAD = {"metal": 2.5, "plastic": 5.0, "nut": 2.0}   # mm engaged, M2.5: 1 diameter in metal, 2 in plastic, a whole nut

HARDWARE = Path(__file__).resolve().parent.parent / "hardware.toml"


def finding(check, part, what, mm=None, at=None):
    return {"check": check, "part": part, "what": what,
            "mm": None if mm is None else round(mm, 2),
            "at": None if at is None else [round(c, 1) for c in at]}


# --- thin ---------------------------------------------------------------------

def thin_walls(shape, part, min_wall=MIN_WALL):
    """Every pair of faces that don't share an edge, closer than min_wall,
    whose outward normals at the closest points point away from each other,
    with solid between them. Catches necks as well as walls: the closest
    points may sit on edges, where two thick pieces meet through a thin ring."""
    faces = shape.faces()
    by_edge = defaultdict(set)
    for i, f in enumerate(faces):
        for e in f.edges():
            by_edge[hash(e)].add(i)
    adjacent = {(a, b) for fl in by_edge.values() for a in fl for b in fl}
    boxes = np.array([[*b.min, *b.max] for b in (f.bounding_box() for f in faces)])
    found = []
    for i in range(len(faces)):
        rest = boxes[i + 1:]
        near = np.all(rest[:, :3] - min_wall <= boxes[i, 3:], axis=1) & np.all(rest[:, 3:] + min_wall >= boxes[i, :3], axis=1)
        for j in np.nonzero(near)[0] + i + 1:
            if (i, j) in adjacent:
                continue
            d, p1, p2 = faces[i].distance_to_with_closest_points(faces[j])
            if not 0.01 < d < min_wall - 1e-6:
                continue
            u = (p2 - p1) / d
            if faces[i].normal_at(p1).dot(u) > -0.5 or faces[j].normal_at(p2).dot(u) < 0.5:
                continue                      # not opposite sides of the same plastic
            mid = (p1 + p2) * 0.5
            if shape.is_inside(mid):
                found.append(finding("thin", part, f"{d:.2f} mm of plastic, under {min_wall} mm", d, mid))
    return merge(found)


def merge(found, within=2.0):
    """One finding per place: a thin spot shows up once per pair of faces."""
    kept = []
    for f in sorted(found, key=lambda f: f["mm"]):
        if not any(k["check"] == f["check"] and math.dist(k["at"], f["at"]) < within for k in kept):
            kept.append(f)
    return kept


# --- hole ---------------------------------------------------------------------

def round_holes(shape):
    """(diameter, a point on its axis) for each full round hole: concave
    cylindrical faces, grouped by axis and radius (a hole is often two
    half-faces), whose arcs add up to a full turn."""
    from OCP.BRepAdaptor import BRepAdaptor_Surface
    from OCP.GeomAbs import GeomAbs_Cylinder
    groups = defaultdict(float)
    where = {}
    for f in shape.faces():
        s = BRepAdaptor_Surface(f.wrapped)
        if s.GetType() != GeomAbs_Cylinder:
            continue
        cyl = s.Cylinder()
        r = cyl.Radius()
        ax = cyl.Axis()
        o, d = ax.Location(), ax.Direction()
        o = Vector(o.X(), o.Y(), o.Z())
        d = Vector(d.X(), d.Y(), d.Z())
        c = f.center()
        radial = (c - o) - d * (c - o).dot(d)
        if f.normal_at(c).dot(radial) >= 0:
            continue                          # convex: a post, not a hole
        # The axis point nearest the origin, and the direction up to sign, name the axis.
        foot = o - d * o.dot(d)
        sign = 1 if (d.X, d.Y, d.Z) > (0, 0, 0) else -1
        key = (round(2 * r, 2), *(round(v, 1) for v in (foot.X, foot.Y, foot.Z)),
               *(round(sign * v, 3) for v in (d.X, d.Y, d.Z)))
        groups[key] += s.LastUParameter() - s.FirstUParameter()
        where.setdefault(key, c - radial)
    return [(k[0], where[k]) for k, turn in groups.items() if turn > 2 * math.pi - 0.05]


def hole_fits(shape, part, fits):
    sizes = {round(f["d"], 2): name for name, f in fits.items()}
    found = []
    for d, at in round_holes(shape):
        if d < FASTENER_HOLE and d not in sizes:
            found.append(finding("hole", part, f"{d:.2f} mm hole isn't a tested fit ({', '.join(f'{v} {k}' for k, v in sizes.items())})", d, at))
    return merge(found, within=0.5)


# --- hardware -----------------------------------------------------------------

def screw_length(name):
    m = re.search(r"x(\d+(?:\.\d+)?)", name)
    return float(m.group(1)) if m else None


def hardware(uses, have):
    """uses: dicts of part (a name in hardware.toml), count, where, and for
    anything threaded: through (mm it passes before it threads), into
    ("metal", "plastic" or "nut"), depth (mm of thread there before it bottoms
    out; none for a nut, which it passes through), and length (mm of thread,
    if the name doesn't say: a standoff's stud)."""
    found = []
    stock = {h["part"]: h for h in have}
    totals = defaultdict(int)
    for u in uses:
        totals[u["part"]] += u["count"]
        if u["part"] not in stock:
            found.append(finding("hardware", u["where"], f"needs {u['count']}× {u['part']}, which isn't on hand"))
            continue
        length = u.get("length") or (screw_length(u["part"]) if u["part"].startswith("screw") else None)
        if length is None or "through" not in u:
            continue
        thread = length - u["through"]
        need = MIN_THREAD[u["into"]]
        if thread < need:
            found.append(finding("hardware", u["where"], f"{u['part']} threads {thread:.1f} mm into {u['into']}, under {need} mm", thread))
        elif "depth" in u and thread > u["depth"]:
            found.append(finding("hardware", u["where"], f"{u['part']} reaches {thread:.1f} mm in, {thread - u['depth']:.1f} mm past the {u['depth']:.1f} mm there", thread))
    for part, n in totals.items():
        if part in stock and n > stock[part]["count"]:
            found.append(finding("hardware", "the build", f"needs {n}× {part}, {stock[part]['count']} on hand"))
    return found


# --- overhang -----------------------------------------------------------------

def overhangs(shape, part, to_bed):
    """On the part as it sits on the bed (to_bed: model -> print), any face
    pointing down steeper than OVERHANG from vertical, off the bed. Points are
    reported back in the model's frame."""
    from OCP.BRepAdaptor import BRepAdaptor_Surface
    from OCP.GeomAbs import GeomAbs_Cylinder, GeomAbs_Plane
    printed = to_bed * shape
    bed = printed.bounding_box().min.Z
    back = to_bed.inverse()
    limit = -math.sin(math.radians(90 - OVERHANG))
    found = []
    for f in printed.faces():
        surface = BRepAdaptor_Surface(f.wrapped)
        kind = surface.GetType()
        if kind == GeomAbs_Cylinder and surface.Cylinder().Radius() < 4:
            continue                          # a small round hole lying down: prints as is
        bb = f.bounding_box()
        if bb.max.Z < bed + 0.05:
            continue                          # on the bed
        if kind == GeomAbs_Plane:
            n = f.normal_at()
            if n.Z > limit:
                continue
            at = (back * Location(f.center())).position
            if n.Z < -0.999:                  # a flat ceiling: bridged if it's narrow
                span = min(bb.size.X, bb.size.Y)
                if span > BRIDGE:
                    found.append(finding("overhang", part, f"flat ceiling {span:.1f} mm across, over {BRIDGE:g} mm", span, at))
            elif f.area > 1.0:
                tilt = math.degrees(math.acos(-n.Z))
                found.append(finding("overhang", part, f"face {90 - tilt:.0f}° from vertical, {f.area:.0f} mm²", 90 - tilt, at))
            continue
        verts, tris = f.tessellate(0.1)
        if not tris:
            continue
        v = np.array([[p.X, p.Y, p.Z] for p in verts])
        t = np.array(tris)
        n = np.cross(v[t[:, 1]] - v[t[:, 0]], v[t[:, 2]] - v[t[:, 0]])
        area = np.linalg.norm(n, axis=1) / 2
        nz = n[:, 2] / np.maximum(2 * area, 1e-12)
        down = (nz < limit) & (v[t].mean(axis=1)[:, 2] > bed + 0.05)
        if area[down].sum() > 2.0:
            c = v[t[down]].mean(axis=(0, 1))
            at = (back * Location(Vector(*c))).position
            found.append(finding("overhang", part, f"{area[down].sum():.0f} mm² of curved face pointing down past {OVERHANG:g}°", area[down].sum(), at))
    return merge(found)


# --- all of it -----------------------------------------------------------------

def load_hardware(path=HARDWARE):
    data = tomllib.loads(path.read_text())
    return data.get("have", []), data.get("fits", {})


def allowed(f, allow):
    """allow: (check, part substring, mm or None, why). Matches within 0.05 mm."""
    for check, part, mm, _why in allow:
        if f["check"] == check and part in f["part"] and (mm is None or (f["mm"] is not None and abs(f["mm"] - mm) <= 0.05)):
            return True
    return False


def run(parts, uses, allow=()):
    """parts: (label, shape in the model's frame, Location model -> print)."""
    have, fits = load_hardware()
    found = []
    for label, shape, to_bed in parts:
        found += thin_walls(shape, label)
        found += hole_fits(shape, label, fits)
        found += overhangs(shape, label, to_bed)
    found += hardware(uses, have)
    return [f for f in found if not allowed(f, allow)]


def describe(f):
    at = f" at ({', '.join(f'{c:g}' for c in f['at'])})" if f["at"] else ""
    return f"✕ {f['check']:<9} {f['part']}: {f['what']}{at}"
