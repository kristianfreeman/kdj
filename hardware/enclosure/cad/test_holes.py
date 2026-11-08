"""A strip to find out what size this printer makes a hole, before any part
depends on it. Printed holes come out a few tenths small, by an amount that
depends on the printer, filament and slicer, so every hole in the model will
take its size from what fits here rather than from a guess.

  Top row: round holes through 3 mm, as in the floor, 2.0 to 3.4 mm. Find the
  smallest an M2.5 x 6 screw drops through (clearance), and the one it bites
  into and threads itself into with firm, even resistance (self-tapping).

  Bottom row: hex pockets 1 mm deep, as under the Pi's standoffs, each with a
  3.0 mm hole through. Find the smallest a standoff presses into without
  turning.

Sizes are debossed beside each one. Prints flat, no supports, in about ten
minutes.

  docker run --rm -v "$PWD:/w" -w /w kdj-cad python cad/test_holes.py   # from hardware/enclosure
"""

from pathlib import Path

from build123d import (Align, Box, Cylinder, Pos, RegularPolygon, Text, export_stl, extrude)

FONT = str(Path(__file__).resolve().parent / "fonts" / "GeistMono-Bold.ttf")
OUT = Path(__file__).resolve().parent.parent / "out"

T = 3.0                       # thick as the floor's pads under the standoffs
PITCH = 9.0                   # hole to hole
ROUND = [2.0 + 0.2 * i for i in range(8)]     # 2.0 ... 3.4
HEX = [5.0, 5.2, 5.4, 5.6]                    # across flats: an M2.5 standoff is usually 5.0
HEX_DEPTH, HEX_HOLE = 1.0, 3.0
TEXT, DEBOSS = 4.0, 0.6
MARGIN = 4.0

ROW_A, ROW_B = 21.0, 7.0      # hole centres; each label sits just above its hole
W = 2 * MARGIN + PITCH * (len(ROUND) - 1) + 2 * 3.0
D = ROW_A + 9.0


def label(text, x, y):
    sketch = Text(text, font_size=TEXT, font_path=FONT, align=(Align.CENTER, Align.CENTER))
    return Pos(x, y, T - DEBOSS) * extrude(sketch, amount=DEBOSS + 0.1)


def strip():
    part = Box(W, D, T, align=(Align.MIN, Align.MIN, Align.MIN))
    x0 = MARGIN + 3.0
    for i, d in enumerate(ROUND):
        x = x0 + i * PITCH
        part -= Pos(x, ROW_A, -1) * Cylinder(d / 2, T + 2, align=(Align.CENTER, Align.CENTER, Align.MIN))
        part -= label(f"{d:.1f}", x, ROW_A + 5.6)
    step = PITCH * (len(ROUND) - 1) / (len(HEX) - 1)
    for i, af in enumerate(HEX):
        x = x0 + i * step
        pocket = RegularPolygon(af / 2, 6, major_radius=False)
        part -= Pos(x, ROW_B, T - HEX_DEPTH) * extrude(pocket, amount=HEX_DEPTH + 0.1)
        part -= Pos(x, ROW_B, -1) * Cylinder(HEX_HOLE / 2, T + 2, align=(Align.CENTER, Align.CENTER, Align.MIN))
        part -= label(f"{af:.1f}", x, ROW_B + 5.6)
    return part


if __name__ == "__main__":
    OUT.mkdir(exist_ok=True)
    part = strip()
    export_stl(part, str(OUT / "test-holes.stl"))
    bb = part.bounding_box()
    print(f"test-holes.stl  {bb.size.X:.1f} x {bb.size.Y:.1f} x {bb.size.Z:.1f} mm, {part.volume:.0f} mm3")
