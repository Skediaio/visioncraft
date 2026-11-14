#!/usr/bin/env python3
"""Writes the 3DCraft command script of the beach house (beach-house.steps.json) and the
name -> feature id table VisionCraft's material script uses (bodies.json).

Python 3 standard library only. Run:  python3 generate.py  (writes beside this file)
Then:  craft3d-cli run --new '{"units":"mm"}' --script beach-house.steps.json --out beach-house.3dcraft

3DCraft axes, millimetres: X east, Y north, Z up. The sea is to the south (-Y).
Origin: the south-west outer corner of the house at ground level (Z 0).
See brief.md for the plan and the room schedule.
"""
import json
import math
import os
import sys

# ---- Parameters -------------------------------------------------------------------------------
W = 24000            # overall width of the house, outside of the cladding (X)
D = 10500            # overall depth (Y)
CEIL = 2850          # finished floor to ceiling
OVERHANG = 900       # roof overhang on the north, east and west
OVERHANG_SEA = 3000  # roof overhang on the sea side: the covered terrace
GLASS_BAYS = 6       # sliding panels in the living room's glass wall
OPEN_BAYS = [3]      # panels slid open (each over its western neighbour)
DECK_DEPTH = 6600    # timber deck south of the house
DECK_SIDE = 600      # how far the deck runs past the house east and west

# ---- Derived levels and plan lines --------------------------------------------------------------
T_EXT, T_CLAD, T_INT = 300, 60, 120   # outer wall (core 240 + cladding 60), inner walls
SLAB_Z0, SLAB_Z1 = 400, 580           # concrete floor slab
FFL = 600                             # finished floor level (20 mm floor finishes on the slab)
CEIL_Z = FFL + CEIL                   # underside of the ceilings
WALL_TOP = CEIL_Z + 20                # top of the walls and ceiling boards; the roof sits on it
ROOF_T = 300
ROOF_Z1 = WALL_TOP + ROOF_T
DOOR_H = 2100                         # internal doors, finished floor to head
I0, IX, IY = T_EXT, W - T_EXT, D - T_EXT  # inside faces of the outer walls

XB = W - 9500        # wall between the hall and the night wing (west face)
XA = XB - 2500       # wall between the hall and the guest WC (west face)
YC = D - 3900        # wall along the north rooms (south face)
YD = YC - 1320       # wall between the night wing's south rooms and the corridor
XG = W - 3600        # wall between the master bedroom and the ensuite
X_DEN = 4400         # den | utility
X_UTIL = XA - 2000   # utility | guest WC
X_BED2 = XB + 3900   # bedroom 2 | bathroom
X_BATH = X_BED2 + 2200  # bathroom | bedroom 3

steps = []
groups = {}          # group name -> list of part feature names
next_id = [1]        # 3DCraft numbers features F1, F2, ... in the order they are made
ids = {}             # feature name -> id


def feature(cmd, params):
    steps.append([cmd, params])
    ids[params["name"]] = f"F{next_id[0]}"
    next_id[0] += 1
    return params["name"]


def mm(v):
    v = round(v, 2)
    return f"{int(v) if v == int(v) else v} mm"


counter = {}


def part(group, x0, y0, z0, x1, y1, z1, name=None):
    """A block between two corners, added to a group."""
    x0, x1 = min(x0, x1), max(x0, x1)
    y0, y1 = min(y0, y1), max(y0, y1)
    z0, z1 = min(z0, z1), max(z0, z1)
    assert x1 - x0 > 0 and y1 - y0 > 0 and z1 - z0 > 0, (group, x0, y0, z0, x1, y1, z1)
    assert -500 <= z0 and z1 <= ROOF_Z1 + 1000, ("z out of range: arguments in the wrong order?", group)
    counter[group] = counter.get(group, 0) + 1
    name = name or f"{group}.{counter[group]}"
    feature("feature.block", {"size": [mm(x1 - x0), mm(y1 - y0), mm(z1 - z0)],
                              "position": [mm(x0), mm(y0), mm(z0)], "name": name})
    groups.setdefault(group, []).append(name)
    return name


def pattern(group, source, count, step):
    """count copies of a part's block, the source included, as one part of the group."""
    groups[group].remove(source)
    name = feature("feature.pattern", {"source": source, "count": count,
                                       "step": [mm(s) for s in step], "name": f"{source}*{count}"})
    groups[group].append(name)
    return name


def union(name, parts):
    """Joins parts into one body called name (the last feature of the chain)."""
    if len(parts) == 1:
        feature("feature.transform", {"source": parts[0], "name": name})
        return name
    acc = parts[0]
    for k, p in enumerate(parts[1:], 1):
        n = name if k == len(parts) - 1 else f"{name}~{k}"
        acc = feature("feature.boolean", {"op": "union", "target": acc, "tool": p, "name": n})
    return acc


# ---- Plinth, slab -------------------------------------------------------------------------------
part("plinth", 150, 150, -300, W - 150, D - 150, SLAB_Z0)   # set back: the house floats
part("slab", 0, 0, SLAB_Z0, W, D, SLAB_Z1)

# ---- Outer walls: core and cladding as rings, openings cut from both ----------------------------
def ring(group, out, inner, z0, z1):
    (ox0, oy0, ox1, oy1), (ix0, iy0, ix1, iy1) = out, inner
    part(group, ox0, oy0, z0, ox1, iy0, z1)   # south
    part(group, ox0, iy1, z0, ox1, oy1, z1)   # north
    part(group, ox0, iy0, z0, ix0, iy1, z1)   # west
    part(group, ix1, iy0, z0, ox1, iy1, z1)   # east


ring("walls-ext", (T_CLAD, T_CLAD, W - T_CLAD, D - T_CLAD), (I0, I0, IX, IY), SLAB_Z1, WALL_TOP)
ring("cladding", (0, 0, W, D), (T_CLAD, T_CLAD, W - T_CLAD, D - T_CLAD), SLAB_Z1, WALL_TOP)


def cut(side, a, b, z0, z1):
    """An opening through the outer wall on side s (S, N, W, E) from a to b along the wall."""
    if side == "S":
        return part("openings", a, -10, z0, b, T_EXT + 10, z1)
    if side == "N":
        return part("openings", a, D - T_EXT - 10, z0, b, D + 10, z1)
    if side == "W":
        return part("openings", -10, a, z0, T_EXT + 10, b, z1)
    return part("openings", W - T_EXT - 10, a, z0, W + 10, b, z1)


def in_wall(side, a, b, d0, d1, z0, z1, group):
    """A block in the plane of an outer wall: a..b along it, d0..d1 measured in from the outside."""
    if side == "S":
        return part(group, a, d0, z0, b, d1, z1)
    if side == "N":
        return part(group, a, D - d1, z0, b, D - d0, z1)
    if side == "W":
        return part(group, d0, a, z0, d1, b, z1)
    return part(group, W - d1, a, z0, W - d0, b, z1)


# Windows: (side, from, to, sill above floor, head above floor).
WINDOWS = [
    ("S", XG + 480, IX - 500, 700, 2400),          # ensuite, over the bath
    ("N", 1200, 3600, 900, 2400),                  # den
    ("N", X_DEN + 1480, X_DEN + 2680, 1200, 2400),  # utility
    ("N", X_UTIL + 480, X_UTIL + 1280, 1500, 2400),  # guest WC
    ("N", XB + 780, XB + 2980, 900, 2400),         # bedroom 2
    ("N", X_BED2 + 480, X_BED2 + 1580, 1500, 2400),  # bathroom
    ("N", X_BATH + 580, X_BATH + 2380, 900, 2400),  # bedroom 3
    ("W", 700, 1500, 0, CEIL),                     # living, floor to ceiling slot south of the fire
    ("W", 5100, YC - 400, 0, CEIL),                # living, slot north of the fire
    ("W", YC + 1000, YC + 3000, 900, 2400),        # den
    ("E", YC + 1000, YC + 3100, 900, 2400),        # bedroom 3
]
FR, FD, FO = 60, 80, 60   # window frame face width, depth, set in from the outside face


def window(side, a, b, sill, head):
    z0 = FFL + sill if sill > 0 else SLAB_Z1
    z1 = FFL + head
    cut(side, a, b, z0, z1)
    d0, d1 = FO, FO + FD
    in_wall(side, a, a + FR, d0, d1, z0, z1, "frames")
    in_wall(side, b - FR, b, d0, d1, z0, z1, "frames")
    in_wall(side, a + FR, b - FR, d0, d1, z1 - FR, z1, "frames")
    in_wall(side, a + FR, b - FR, d0, d1, z0, z0 + FR, "frames")
    lites = max(1, math.ceil((b - a) / 1400))
    span = (b - a - 2 * FR - (lites - 1) * FR) / lites
    for k in range(lites):
        l0 = a + FR + k * (span + FR)
        if k > 0:
            in_wall(side, l0 - FR, l0, d0, d1, z0 + FR, z1 - FR, "frames")   # mullion
        # 10 mm glass in the middle of the frame, 15 mm into its rebates
        in_wall(side, l0 - 15, l0 + span + 15, d0 + 35, d0 + 45, z0 + FR - 15, z1 - FR + 15, "glass")
    if sill > 0:   # an external metal sill, standing 50 mm proud of the cladding
        in_wall(side, a - 30, b + 30, -50, d0 + 10, z0 - 40, z0, "sills")


for w in WINDOWS:
    window(*w)


def sliding_wall(a, b, bays, open_bays, prefix):
    """A floor-to-ceiling sliding glass wall in the south wall from a to b: an outer frame with a
    head and a sill track, and one framed panel per bay on two tracks, so a panel slides over its
    neighbour."""
    z0, z1 = SLAB_Z1, WALL_TOP
    cut("S", a, b, z0, z1)
    J, d0, d1 = 80, 40, 260
    in_wall("S", a, a + J, d0, d1, z0, z1, "frames")
    in_wall("S", b - J, b, d0, d1, z0, z1, "frames")
    in_wall("S", a + J, b - J, d0, d1, z1 - 120, z1, "frames")    # head track
    in_wall("S", a + J, b - J, d0, d1, z0, FFL + 20, "frames")     # sill track
    w = (b - a - 2 * J) / bays
    pz0, pz1 = FFL + 20, z1 - 120
    for i in range(bays):
        x0 = a + J + i * w - (25 if i > 0 else 0)
        x1 = a + J + (i + 1) * w + (25 if i < bays - 1 else 0)
        if i in open_bays and i > 0:
            x0, x1 = x0 - w + 50, x1 - w + 50
        t0 = 70 if i % 2 else 190            # outer or inner track
        t1 = t0 + 50
        S, top, bottom = 50, 50, 90
        in_wall("S", x0, x0 + S, t0, t1, pz0, pz1, "frames")
        in_wall("S", x1 - S, x1, t0, t1, pz0, pz1, "frames")
        in_wall("S", x0 + S, x1 - S, t0, t1, pz1 - top, pz1, "frames")
        in_wall("S", x0 + S, x1 - S, t0, t1, pz0, pz0 + bottom, "frames")
        in_wall("S", x0 + S - 15, x1 - S + 15, t0 + 20, t0 + 30, pz0 + bottom - 15, pz1 - top + 15, "glass")


sliding_wall(450, XB - 150, GLASS_BAYS, OPEN_BAYS, "living")
sliding_wall(XB + 400, XG - 300, 2, [], "master")

# Front door: a pivoting timber leaf and a sidelight in a dark frame.
FD_A, FD_B, FD_TOP = XA + 500, XB - 400, FFL + 2400
cut("N", FD_A, FD_B, SLAB_Z1, FD_TOP)
in_wall("N", FD_A, FD_A + FR, FO, FO + FD, SLAB_Z1, FD_TOP, "frames")
in_wall("N", FD_B - FR, FD_B, FO, FO + FD, SLAB_Z1, FD_TOP, "frames")
in_wall("N", FD_A + FR, FD_B - FR, FO, FO + FD, FD_TOP - FR, FD_TOP, "frames")
LEAF_B = FD_A + FR + 1100
in_wall("N", LEAF_B, LEAF_B + FR, FO, FO + FD, SLAB_Z1, FD_TOP - FR, "frames")   # mullion
in_wall("N", FD_A + FR + 5, LEAF_B - 5, FO + 10, FO + 70, FFL + 5, FD_TOP - FR - 5, "doors")
in_wall("N", LEAF_B + FR - 15, FD_B - FR + 15, FO + 35, FO + 45, FFL, FD_TOP - FR + 15, "glass")
in_wall("N", LEAF_B - 160, LEAF_B - 130, -40, FO + 10, FFL + 500, FFL + 1900, "hardware")  # pull bar
in_wall("N", FD_A + FR, FD_B - FR, FO, T_EXT, SLAB_Z1, FFL, "sills")   # threshold

union("openings-all", groups.pop("openings"))
feature("feature.boolean", {"op": "subtract", "target": union("walls-ext-solid", groups.pop("walls-ext")),
                            "tool": "openings-all", "keepTool": True, "name": "walls-ext"})
feature("feature.boolean", {"op": "subtract", "target": union("cladding-solid", groups.pop("cladding")),
                            "tool": "openings-all", "name": "cladding"})
FINISHED = ["walls-ext", "cladding"]

# ---- Inner walls and doors ----------------------------------------------------------------------
INNER = [  # x0, y0, x1, y1
    (I0, YC, XA, YC + T_INT),                    # C: open plan | north rooms of the day wing
    (XA, YC, XA + T_INT, IY),                    # A: guest WC | hall
    (X_DEN, YC + T_INT, X_DEN + T_INT, IY),      # den | utility
    (X_UTIL, YC + T_INT, X_UTIL + T_INT, IY),    # utility | guest WC
    (XB, I0, XB + T_INT, IY),                    # B: day | night wing
    (XB + T_INT, YD, IX, YD + T_INT),            # D: south rooms | corridor
    (XB + T_INT, YC, IX, YC + T_INT),            # E: corridor | north rooms
    (XG, I0, XG + T_INT, YD),                    # G: master | ensuite
    (X_BED2, YC + T_INT, X_BED2 + T_INT, IY),    # bedroom 2 | bathroom
    (X_BATH, YC + T_INT, X_BATH + T_INT, IY),    # bathroom | bedroom 3
]
for x0, y0, x1, y1 in INNER:
    part("walls-int", x0, y0, SLAB_Z1, x1, y1, CEIL_Z)

# Doors: (wall axis, wall line (low face), from, swing side +1/-1, room the threshold belongs to).
DW, LEAF_T, LIN = 910, 40, 30
DOORS = [
    ("y", YC, 3000, +1, "den"),
    ("y", YC, X_DEN + 300, +1, "utility"),
    ("x", XA, IY - 1100, -1, "wc"),
    ("x", XB, YD + 270, +1, "corridor"),
    ("y", YD, XB + 180, -1, "master"),
    ("x", XG, YD - 1380, +1, "ensuite"),
    ("y", YC, X_BED2 - 1100, +1, "bed2"),
    ("y", YC, X_BED2 + 580, +1, "bath"),
    ("y", YC, X_BATH + 100, +1, "bed3"),
]
thresholds = {}
for axis, line, a, swing, room in DOORS:
    b = a + DW
    top = FFL + DOOR_H
    lo, hi = line, line + T_INT
    if axis == "y":   # wall runs along X
        part("door-openings", a, lo - 10, SLAB_Z1, b, hi + 10, top)
        part("door-frames", a, lo - 10, FFL, a + LIN, hi + 10, top)
        part("door-frames", b - LIN, lo - 10, FFL, b, hi + 10, top)
        part("door-frames", a + LIN, lo - 10, top - LIN, b - LIN, hi + 10, top)
        y0, y1 = (hi, hi + (DW - 2 * LIN)) if swing > 0 else (lo - (DW - 2 * LIN), lo)
        part("doors", a + LIN, y0, FFL + 10, a + LIN + LEAF_T, y1, top - LIN - 5)   # open 90 degrees
        thresholds.setdefault(room, []).append((a + LIN, lo, b - LIN, hi))
    else:             # wall runs along Y
        part("door-openings", lo - 10, a, SLAB_Z1, hi + 10, b, top)
        part("door-frames", lo - 10, a, FFL, hi + 10, a + LIN, top)
        part("door-frames", lo - 10, b - LIN, FFL, hi + 10, b, top)
        part("door-frames", lo - 10, a + LIN, top - LIN, hi + 10, b - LIN, top)
        x0, x1 = (hi, hi + (DW - 2 * LIN)) if swing > 0 else (lo - (DW - 2 * LIN), lo)
        part("doors", x0, a + LIN, FFL + 10, x1, a + LIN + LEAF_T, top - LIN - 5)
        thresholds.setdefault(room, []).append((lo, a + LIN, hi, b - LIN))
feature("feature.boolean", {"op": "subtract", "target": union("walls-int-solid", groups.pop("walls-int")),
                            "tool": union("door-openings-all", groups.pop("door-openings")), "name": "walls-int"})
FINISHED.append("walls-int")

# ---- Floors per room (20 mm finishes on the slab) -----------------------------------------------
ROOMS = {  # name: (label, rectangle x0, y0, x1, y1)
    "living":   ("Living, dining, kitchen", (I0, I0, XB, YC)),
    "hall":     ("Entrance hall", (XA + T_INT, YC, XB, IY)),
    "den":      ("Den", (I0, YC + T_INT, X_DEN, IY)),
    "utility":  ("Utility and pantry", (X_DEN + T_INT, YC + T_INT, X_UTIL, IY)),
    "wc":       ("Guest WC", (X_UTIL + T_INT, YC + T_INT, XA, IY)),
    "corridor": ("Corridor", (XB + T_INT, YD + T_INT, IX, YC)),
    "master":   ("Master bedroom", (XB + T_INT, I0, XG, YD)),
    "ensuite":  ("Ensuite", (XG + T_INT, I0, IX, YD)),
    "bed2":     ("Bedroom 2", (XB + T_INT, YC + T_INT, X_BED2, IY)),
    "bath":     ("Bathroom", (X_BED2 + T_INT, YC + T_INT, X_BATH, IY)),
    "bed3":     ("Bedroom 3", (X_BATH + T_INT, YC + T_INT, IX, IY)),
}
thresholds.setdefault("living", []).append((450, 140, XB - 150, I0))        # under the glass wall
thresholds.setdefault("master", []).append((XB + 400, 140, XG - 300, I0))
thresholds.setdefault("hall", []).append((FD_A + FR, IY, FD_B - FR, D - FO - FD))
for room, (_, (x0, y0, x1, y1)) in ROOMS.items():
    part(f"floor-{room}", x0, y0, SLAB_Z1, x1, y1, FFL)
    for t in thresholds.get(room, []):
        part(f"floor-{room}", t[0], t[1], SLAB_Z1, t[2], t[3], FFL)

# ---- Ceilings -----------------------------------------------------------------------------------
part("ceiling-living", I0, I0, CEIL_Z, XB, YC, WALL_TOP)             # timber, as the soffit outside
part("ceiling", I0, YC, CEIL_Z, XB, IY, WALL_TOP)
part("ceiling", XB, I0, CEIL_Z, IX, IY, WALL_TOP)

# ---- Roof: slab with a deep overhang, membrane, fascia with a low upstand, chimney ---------------
RX0, RY0, RX1, RY1 = -OVERHANG, -OVERHANG_SEA, W + OVERHANG, D + OVERHANG
part("roof", RX0, RY0, WALL_TOP, RX1, RY1, ROOF_Z1)
part("roof-membrane", RX0, RY0, ROOF_Z1, RX1, RY1, ROOF_Z1 + 20)
F_T, F_Z0, F_Z1 = 60, WALL_TOP - 50, ROOF_Z1 + 120
ring("fascia", (RX0 - F_T, RY0 - F_T, RX1 + F_T, RY1 + F_T), (RX0, RY0, RX1, RY1), F_Z0, F_Z1)
for x in (-DECK_SIDE + 100, W + DECK_SIDE - 250):     # slim steel posts at the terrace corners
    part("columns", x, RY0 + 150, SLAB_Z1, x + 150, RY0 + 300, WALL_TOP)

# ---- Fireplace on the living room's west wall ---------------------------------------------------
part("fireplace", I0, 2200, FFL, I0 + 450, 4700, CEIL_Z)                 # chimney breast to the ceiling
part("fireplace", I0 + 450, 1200, FFL, I0 + 900, 5700, FFL + 400)       # stone hearth bench
part("fireplace", I0 - 50, 2900, ROOF_Z1 + 20, I0 + 550, 4000, ROOF_Z1 + 900)  # stack on the roof
part("firebox", I0 + 450, 2900, FFL + 400, I0 + 460, 4000, FFL + 950)    # closed insert, black glass

# ---- Kitchen: counter run with tall units on the north wall, island with a waterfall top --------
KX0, KX1, TALL = 6000, XA - 1200, XA
part("kitchen-units", KX0, YC - 520, FFL, KX1, YC, FFL + 100)            # recessed plinth
part("kitchen-units", KX0, YC - 600, FFL + 100, KX1, YC, FFL + 860)      # base units
part("worktops", KX0, YC - 620, FFL + 860, KX1, YC, FFL + 900)
part("kitchen-units", KX1, YC - 600, FFL, TALL, YC, FFL + 2400)          # tall units, ovens
part("kitchen-units", KX0 + 600, YC - 300, FFL + 1600, KX1 - 600, YC, FFL + 1640)  # open shelf
IX0, IX1, IY0, IY1 = 6900, 9900, YC - 2900, YC - 2000
part("kitchen-units", IX0 + 40, IY0 + 80, FFL, IX1 - 40, IY1, FFL + 100)
part("kitchen-units", IX0 + 40, IY0, FFL + 100, IX1 - 40, IY1, FFL + 860)
part("worktops", IX0, IY0 - 300, FFL + 860, IX1, IY1, FFL + 900)        # 300 overhang for stools
part("worktops", IX0, IY0 - 300, FFL, IX0 + 40, IY1, FFL + 860)         # waterfall ends
part("worktops", IX1 - 40, IY0 - 300, FFL, IX1, IY1, FFL + 860)
part("appliances", 7800, IY0 + 100, FFL + 900, 8600, IY0 + 620, FFL + 905)  # induction hob
part("appliances", 8400, YC - 560, FFL + 900, 9200, YC - 140, FFL + 902)    # sink, dark steel
part("appliances", KX1 + 300, YC - 605, FFL + 900, KX1 + 900, YC - 600, FFL + 1500)  # oven front
# utility: a counter run and a larder on the north wall
UX0 = X_DEN + T_INT
part("kitchen-units", UX0 + 1500, IY - 600, FFL, X_UTIL, IY, FFL + 860)
part("worktops", UX0 + 1500, IY - 620, FFL + 860, X_UTIL, IY, FFL + 900)
part("kitchen-units", UX0, IY - 600, FFL, UX0 + 1500, IY, FFL + 2400)

# ---- Built-in wardrobes and shelving (to the ceiling) -------------------------------------------
part("joinery", XB + 1400, YD - 600, FFL, XG - 600, YD, CEIL_Z)          # master
part("joinery", XB + T_INT, YC + T_INT, FFL, XB + 2120, YC + 720, CEIL_Z)  # bedroom 2
part("joinery", X_BATH + T_INT + 1200, YC + T_INT, FFL, IX, YC + 720, CEIL_Z)  # bedroom 3
part("joinery", XA + T_INT, YC + 200, FFL, XA + 720, IY - 1300, CEIL_Z)  # hall coat closet
part("joinery", IX - 600, YD + T_INT, FFL, IX, YC, CEIL_Z)              # linen, corridor end
part("joinery", X_DEN - 300, YC + 600, FFL, X_DEN, IY - 400, FFL + 2400)  # den shelving

# ---- Bathrooms: baths, vanities, WCs, mirrors, a shower screen ----------------------------------
def bath(x0, y0, x1, y1):
    part("bath-solid", x0, y0, FFL, x1, y1, FFL + 550)
    part("bath-void", x0 + 100, y0 + 100, FFL + 150, x1 - 100, y1 - 100, FFL + 600)


def vanity(x0, y0, x1, y1, basins, along):
    part("joinery", x0, y0, FFL + 450, x1, y1, FFL + 860)           # wall-hung drawer unit
    part("worktops", x0, y0, FFL + 860, x1, y1, FFL + 880)
    n = len(basins)
    for c in basins:   # vessel basins on the top
        if along == "y":
            part("sanitary", x0 + 60, c - 225, FFL + 880, x1 - 60, c + 225, FFL + 1020)
        else:
            part("sanitary", c - 225, y0 + 60, FFL + 880, c + 225, y1 - 60, FFL + 1020)


def wc(x0, y0, x1, y1, wall):
    """A wall-hung pan in front of a built-out cistern ledge on the given wall side."""
    if wall == "N":
        part("sanitary-ledge", x0, y1 - 200, FFL, x1, y1, FFL + 1200)
        cx = (x0 + x1) / 2
        part("sanitary", cx - 180, y1 - 740, FFL + 80, cx + 180, y1 - 200, FFL + 420)
    elif wall == "W":
        part("sanitary-ledge", x0, y0, FFL, x0 + 200, y1, FFL + 1200)
        cy = (y0 + y1) / 2
        part("sanitary", x0 + 200, cy - 180, FFL + 80, x0 + 740, cy + 180, FFL + 420)
    else:
        part("sanitary-ledge", x1 - 200, y0, FFL, x1, y1, FFL + 1200)
        cy = (y0 + y1) / 2
        part("sanitary", x1 - 740, cy - 180, FFL + 80, x1 - 200, cy + 180, FFL + 420)


# ensuite (XG+120 .. IX, I0 .. YD)
EX0 = XG + T_INT
bath(IX - 1900, I0 + 300, IX - 200, I0 + 1100)
vanity(IX - 550, I0 + 1900, IX, I0 + 3500, [I0 + 2300, I0 + 3100], "y")
part("mirrors", IX - 10, I0 + 1950, FFL + 1200, IX, I0 + 3450, FFL + 2100)
wc(EX0 + 200, YD - 900, EX0 + 1200, YD, "N")
part("glass", IX - 1400, YD - 1200, FFL, IX - 500, YD - 1190, FFL + 2000)   # walk-in shower screen
# bathroom (X_BED2+120 .. X_BATH, YC+120 .. IY)
BX0 = X_BED2 + T_INT
bath(BX0, IY - 750, BX0 + 1700, IY)
vanity(BX0, YC + 1100, BX0 + 500, YC + 2100, [YC + 1600], "y")
part("mirrors", BX0, YC + 1150, FFL + 1200, BX0 + 10, YC + 2050, FFL + 2000)
wc(X_BATH - 200, YC + 1400, X_BATH, YC + 2400, "E")
# guest WC (X_UTIL+120 .. XA, YC+120 .. IY)
GX0 = X_UTIL + T_INT
wc(GX0, YC + 900, GX0 + 200, YC + 1900, "W")
vanity(GX0 + 300, IY - 450, GX0 + 1100, IY, [GX0 + 700], "x")
part("mirrors", GX0 + 350, IY - 10, FFL + 1100, GX0 + 1050, IY, FFL + 1900)

feature("feature.boolean", {"op": "subtract", "target": union("sanitary-baths", groups.pop("bath-solid")),
                            "tool": union("bath-voids", groups.pop("bath-void")), "name": "baths"})
FINISHED.append("baths")

# ---- Deck, steps, glass balustrade, outdoor kitchen, fire pit -----------------------------------
DX0, DX1, DY0 = -DECK_SIDE, W + DECK_SIDE, -DECK_DEPTH
part("deck-base", DX0 + 40, DY0 + 40, -100, DX1 - 40, 0, SLAB_Z1 - 28)
boards = round(DECK_DEPTH / 146)
pitch = DECK_DEPTH / boards
pattern("deck", part("deck", DX0, DY0, SLAB_Z1 - 28, DX1, DY0 + pitch - 5, SLAB_Z1), boards, [0, pitch, 0])
ST0, ST1 = 5400, 10200                       # steps down to the beach, in front of the living room
RISE, GOING = (SLAB_Z1 + 0) / 4, 350
for k in range(1, 4):
    part("steps", ST0, DY0 - k * GOING, -100, ST1, DY0 - (k - 1) * GOING, SLAB_Z1 - k * RISE)
# frameless glass balustrade: 12 mm panels in a base shoe
SHOE, GL_H = 100, 1100


def balustrade(axis, line, a, b, outside):
    n = max(1, math.ceil((b - a) / 1500))
    L = (b - a) / n
    if axis == "x":   # runs along X at y = line, shoe on the deck side of the line
        part("railing", a, line, SLAB_Z1, b, line + 60 * outside, SLAB_Z1 + SHOE)
        for k in range(n):
            part("glass", a + k * L + 10, line + 24 * outside, SLAB_Z1 + 20, a + (k + 1) * L - 10,
                 line + 36 * outside, SLAB_Z1 + GL_H)
    else:
        part("railing", line, a, SLAB_Z1, line + 60 * outside, b, SLAB_Z1 + SHOE)
        for k in range(n):
            part("glass", line + 24 * outside, a + k * L + 10, SLAB_Z1 + 20, line + 36 * outside,
                 a + (k + 1) * L - 10, SLAB_Z1 + GL_H)


balustrade("x", DY0, DX0, ST0, +1)
balustrade("x", DY0, ST1, DX1, +1)
balustrade("y", DX0, DY0 + 60, 0, +1)
balustrade("y", DX1, DY0 + 60, 0, -1)
# outdoor kitchen against the west balustrade, under the roof
OK0, OK1 = -2900, -300
part("kitchen-units", DX0 + 150, OK0 + 60, SLAB_Z1, DX0 + 790, OK1 - 60, SLAB_Z1 + 100)
part("kitchen-units", DX0 + 150, OK0, SLAB_Z1 + 100, DX0 + 850, OK1, SLAB_Z1 + 860)
part("worktops", DX0 + 130, OK0 - 20, SLAB_Z1 + 860, DX0 + 870, OK1 + 20, SLAB_Z1 + 900)
part("appliances", DX0 + 250, OK0 + 500, SLAB_Z1 + 900, DX0 + 750, OK0 + 1300, SLAB_Z1 + 960)  # grill
# fire pit: a concrete block with a bowl of dark stones, out on the open deck
FX, FY = W - 6000, DY0 + 1500
part("firepit-solid", FX, FY, SLAB_Z1, FX + 1200, FY + 1200, SLAB_Z1 + 400)
part("firepit-bowl", FX + 200, FY + 200, SLAB_Z1 + 200, FX + 1000, FY + 1000, SLAB_Z1 + 410)
part("firebox", FX + 200, FY + 200, SLAB_Z1 + 200, FX + 1000, FY + 1000, SLAB_Z1 + 300)
feature("feature.boolean", {"op": "subtract", "target": union("firepit-block", groups.pop("firepit-solid")),
                            "tool": union("firepit-bowls", groups.pop("firepit-bowl")), "name": "firepit"})
FINISHED.append("firepit")

# ---- Carport, entrance landing and steps, paths -------------------------------------------------
CX0, CX1, CY0, CY1 = -6600, -600, D - 6000, D
part("carport-pad", CX0, CY0, -60, CX1, CY1, 20)
for x in (CX0 + 60, CX1 - 180):
    for y in (CY0 + 60, CY1 - 180):
        part("columns", x, y, 20, x + 120, y + 120, 2700)
part("roof", CX0 - 300, CY0 - 300, 2700, CX1 + 300, CY1 + 300, 2950)
part("roof-membrane", CX0 - 300, CY0 - 300, 2950, CX1 + 300, CY1 + 300, 2965)
ring("fascia", (CX0 - 340, CY0 - 340, CX1 + 340, CY1 + 340), (CX0 - 300, CY0 - 300, CX1 + 300, CY1 + 300),
     2650, 3020)
part("cladding-screen", CX0, CY0 + 200, 20, CX0 + 40, CY1 - 200, 2700)   # timber screen, west side
LX0, LX1 = XA - 200, XB + 300
part("entrance", LX0, D, -100, LX1, D + 1800, SLAB_Z1)                    # landing at the door
for k in range(1, 4):
    part("entrance", LX0 + 400, D + 1800 + (k - 1) * 300, -100, LX1 - 400, D + 1800 + k * 300,
         SLAB_Z1 - k * RISE)
PATH_Y = D + 600
pv = part("path", CX1, PATH_Y, -20, CX1 + 900, PATH_Y + 1200, 30)
pattern("path", pv, math.floor((LX0 - CX1) / 960), [960, 0, 0])
pv = part("path", LX0 + 550, D + 2800, -20, LX1 - 550, D + 3400, 30)
pattern("path", pv, 8, [0, 660, 0])

# ---- Join each group into one body, and look it over -------------------------------------------
for g, parts in groups.items():
    union(g, parts)
    FINISHED.append(g)
FINISHED.remove("cladding-screen")
feature("feature.boolean", {"op": "union", "target": "cladding", "tool": "cladding-screen", "name": "cladding+"})
FINISHED[FINISHED.index("cladding")] = "cladding+"

# Colours for 3DCraft's own view (VisionCraft gives materials per body instead).
COLOURS = {
    "walls-ext": "#ece8e0", "walls-int": "#f2efe9", "cladding+": "#8a6a4f", "plinth": "#9a968f",
    "slab": "#a8a49c", "frames": "#2b2d2f", "sills": "#3a3c3e", "doors": "#a27b55",
    "hardware": "#2b2d2f", "door-frames": "#e8e4dc", "ceiling": "#f5f3ef", "ceiling-living": "#b8936b",
    "roof": "#c9a77f", "roof-membrane": "#6e7073", "fascia": "#2b2d2f", "columns": "#2b2d2f",
    "fireplace": "#cfc6b5", "firebox": "#151515", "kitchen-units": "#5f6b5d", "worktops": "#e6e2da",
    "appliances": "#1d1d1d", "joinery": "#c2a27f", "sanitary": "#fafafa", "baths": "#fafafa",
    "mirrors": "#b9c3c8", "sanitary-ledge": "#dcd8d0", "deck-base": "#4a3a2c", "deck": "#9c7a58",
    "steps": "#9c7a58", "railing": "#9a9ea2", "carport-pad": "#a8a49c", "entrance": "#a8a49c",
    "path": "#b5b0a6", "firepit": "#8f8b84",
}
for n in FINISHED:
    if n.startswith("floor-"):
        wet = n in ("floor-ensuite", "floor-bath", "floor-wc", "floor-utility", "floor-hall")
        steps.append(["body.appearance", {"body": n, "color": "#d8d4cc" if wet else "#b48a5e"}])
    elif n == "glass":
        steps.append(["body.appearance", {"body": n, "color": "#dfe8ea", "transmission": 0.9,
                                          "roughness": 0.05, "thinWalled": True}])
    else:
        steps.append(["body.appearance", {"body": n, "color": COLOURS[n]}])
steps.append(["document.inspect", {}])

here = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(here, "..", "..", ".."))

# ---- The VisionCraft example: materials per body, furniture, lights, viewpoints ----------------
# World: metres, Y up, x = X / 1000, y = Z / 1000, z = -Y / 1000 (the house's SW corner at the
# origin, the sea side along z = 0, the deck towards +z, the street towards -z).
def w(x, y, z=FFL):
    return [round(x / 1000, 3), round(z / 1000, 3), round(-y / 1000, 3)]


LIB = {m["id"]: m for m in json.load(open(os.path.join(ROOT, "assets", "materials.json")))["materials"]}
vc = []
mat_ids = {}


def lib_material(lib_id, gloss=0.0):
    """A project material showing a library material, made as the app's Library tab makes one
    (metallic 0: the maps scale the factors)."""
    e = LIB[lib_id]
    scale = max(e["size_m"]) if e.get("size_m") else 1.0
    vc.append(["material.add", {
        "name": e["name"],
        "kind": {"type": "standard_triplanar_pbr", "map_scale_m": scale, "relief": 1.0, "gloss": gloss,
                 "weathering": 0.0},
        "base_color": [1.0, 1.0, 1.0, 1.0], "metallic": 0.0, "roughness": 1.0,
        "maps": {"base_color": e["maps"]["basecolor"], "normal": e["maps"]["normal"], "orm": e["maps"]["orm"],
                 "height": e["maps"].get("height")},
        "variation": 0.4 if e["kind"] == "paving" else 0.25}])
    mat_ids[lib_id] = f"mat{len(mat_ids) + 1}"
    return mat_ids[lib_id]


def plain_material(key, name, rgb, metallic, roughness, gloss=0.0):
    vc.append(["material.add", {"name": name, "base_color": rgb + [1.0], "metallic": metallic,
                                "roughness": roughness,
                                "kind": {"type": "standard_triplanar_pbr", "map_scale_m": 1, "relief": 0,
                                         "gloss": gloss, "weathering": 0}}])
    mat_ids[key] = f"mat{len(mat_ids) + 1}"
    return mat_ids[key]


SAND = lib_material("ground052")
OAK = lib_material("woodfloor032", gloss=0.3)
CEDAR = lib_material("planks007")
DECKING = lib_material("planks029s")
TILE = lib_material("tiles038", gloss=0.4)
HEX = lib_material("tiles071", gloss=0.4)
LIMESTONE = lib_material("rock004")
STONE = lib_material("rocks025")
BOARD_CONCRETE = lib_material("concrete045")
PAVER = lib_material("concrete034")
BRONZE = plain_material("bronze", "Dark bronze metal", [0.035, 0.032, 0.03], 1.0, 0.4)
MEMBRANE = plain_material("membrane", "Roof membrane", [0.1, 0.1, 0.105], 0.0, 0.85)
WHITE = plain_material("white", "White paint", [0.78, 0.77, 0.74], 0.0, 0.7)
LACQUER = plain_material("lacquer", "White lacquer", [0.74, 0.73, 0.7], 0.0, 0.3, gloss=0.3)
SAGE = plain_material("sage", "Sage lacquer", [0.16, 0.2, 0.15], 0.0, 0.4, gloss=0.2)
QUARTZ = plain_material("quartz", "White quartz", [0.8, 0.79, 0.76], 0.0, 0.2, gloss=0.5)
BLACK = plain_material("black", "Black glass", [0.01, 0.01, 0.01], 0.0, 0.08, gloss=0.8)
CERAMIC = plain_material("ceramic", "White ceramic", [0.85, 0.85, 0.84], 0.0, 0.08, gloss=0.8)
MIRROR = plain_material("mirror", "Mirror", [0.92, 0.92, 0.92], 1.0, 0.02)
DARK_WOOD = plain_material("darkwood", "Dark stained timber", [0.045, 0.032, 0.024], 0.0, 0.8)
MATERIALS = {   # body -> material id (built-in ones by name)
    "walls-ext": "plaster", "walls-int": "plaster", "cladding+": CEDAR, "plinth": BOARD_CONCRETE,
    "slab": BOARD_CONCRETE, "roof": CEDAR, "roof-membrane": MEMBRANE, "fascia": BRONZE,
    "columns": BRONZE, "ceiling": WHITE, "ceiling-living": CEDAR, "glass": "glass", "frames": BRONZE,
    "sills": BRONZE, "doors": OAK, "door-frames": WHITE, "hardware": BRONZE,
    "floor-living": OAK, "floor-hall": LIMESTONE, "floor-den": OAK, "floor-utility": TILE,
    "floor-wc": HEX, "floor-corridor": OAK, "floor-master": OAK, "floor-ensuite": TILE,
    "floor-bed2": OAK, "floor-bath": HEX, "floor-bed3": OAK,
    "fireplace": STONE, "firebox": BLACK, "kitchen-units": SAGE, "worktops": QUARTZ,
    "appliances": BLACK, "joinery": LACQUER, "sanitary": CERAMIC, "baths": CERAMIC,
    "sanitary-ledge": TILE, "mirrors": MIRROR, "deck": DECKING, "deck-base": DARK_WOOD,
    "steps": DECKING, "railing": "metal", "firepit": "concrete", "carport-pad": "concrete",
    "entrance": "concrete", "path": PAVER,
}
assert set(MATERIALS) == set(FINISHED), set(MATERIALS) ^ set(FINISHED)
vc.append(["terrain.create", {"origin": [-30, -30], "width_m": 80, "depth_m": 60, "cell_m": 1.0,
                              "base_material": SAND}])
vc.append(["model.add", {"name": "Beach house", "kind": "3dcraft", "source": "example:beach-house.3dcraft",
                         "transform": {"translation": [0, 0, 0], "rotation_y_deg": 0, "scale": 1}}])
for body in FINISHED:
    vc.append(["material.assign", {"target": {"type": "body", "owner": "m1", "body": ids[body]},
                                   "material": MATERIALS[body]}])

# Furniture from the catalogue: (name, asset, X, Y in mm, rotation in degrees, floor Z).
# rotation_y_deg turns counter-clockwise seen from above.
DECK_Z = SLAB_Z1
LIV_Y = (I0 + YC) / 2
FURNITURE = [
    ("Sofa", "sofa_03", 4300, LIV_Y, -90, FFL),
    ("Lounge chair north", "mid_century_lounge_chair", 2300, YC - 1100, 160, FFL),
    ("Lounge chair south", "mid_century_lounge_chair", 2300, I0 + 1300, 20, FFL),
    ("Coffee table", "modern_coffee_table_02", 2900, LIV_Y, 0, FFL),
    ("Plant by the fire", "potted_plant_01", I0 + 600, YC - 500, 0, FFL),
    ("Plant by the glass", "potted_plant_02", 5600, I0 + 700, 0, FFL),
    ("Dining table", "dining_table", 12100, LIV_Y - 300, 0, FFL),
]
for k, x in enumerate((11400, 12100, 12800)):
    FURNITURE.append((f"Dining chair south {k + 1}", "dining_chair_02", x, LIV_Y - 1100, 180, FFL))
    FURNITURE.append((f"Dining chair north {k + 1}", "dining_chair_02", x, LIV_Y + 500, 0, FFL))
for k, x in enumerate((7600, 8400, 9200)):
    FURNITURE.append((f"Bar stool {k + 1}", "bar_chair_round_01", x, IY0 - 600, 0, FFL))
FURNITURE += [
    ("Den sofa", "sofa_02", 1900, IY - 700, 180, FFL),
    ("Den table", "coffee_table_round_01", 1900, YC + 1600, 0, FFL),
    ("Master daybed", "vintage_day_bed", (XB + XG) / 2, YD - 2000, 180, FFL),
    ("Master nightstand", "classicnightstand_01", (XB + XG) / 2 - 1400, YD - 1300, 180, FFL),
    ("Master armchair", "armchair_01", XG - 900, I0 + 1100, -30, FFL),
    ("Bedroom 2 daybed", "vintage_day_bed", XB + 2100, IY - 700, 180, FFL),
    ("Bedroom 2 nightstand", "painted_wooden_nightstand", XB + 3400, IY - 400, 180, FFL),
    ("Bedroom 3 desk", "woodentable_03", X_BATH + 1500, IY - 450, 180, FFL),
    ("Bedroom 3 chair", "greenchair_01", X_BATH + 1500, IY - 1100, 0, FFL),
    ("Bedroom 3 lamp", "desk_lamp_arm_01", X_BATH + 2000, IY - 350, 180, FFL + 830),
    ("Hall plant", "potted_plant_01", XB - 500, IY - 600, 0, FFL),
    ("Terrace table", "round_wooden_table_01", 2600, -1700, 0, DECK_Z),
]
for k, (dx, dy, r) in enumerate(((-1000, 0, 90), (1000, 0, -90), (0, -1000, 0), (0, 1000, 180))):
    FURNITURE.append((f"Terrace chair {k + 1}", "gallinera_chair", 2600 + dx, -1700 + dy, r, DECK_Z))
for k, (dx, dy, r) in enumerate(((-1300, 0, 90), (1300, 0, -90), (0, -1300, 0))):
    FURNITURE.append((f"Fire pit chair {k + 1}", "rockingchair_01", FX + 600 + dx, FY + 600 + dy, r, DECK_Z))
FURNITURE += [
    ("Deck lounge chair 1", "mid_century_lounge_chair", XB + 1500, -1700, 0, DECK_Z),
    ("Deck lounge chair 2", "mid_century_lounge_chair", XB + 3000, -1700, 0, DECK_Z),
    ("Deck side table", "side_table_01", XB + 2250, -1500, 0, DECK_Z),
    ("Planter west", "planter_box_02", 3000, DY0 + 500, 0, DECK_Z),
    ("Planter east", "planter_box_02", W - 2000, DY0 + 500, 0, DECK_Z),
    ("Pot by the steps", "potted_plant_02", ST0 - 600, DY0 + 600, 0, DECK_Z),
    ("Pot at the door", "potted_plant_01", LX1 - 300, D + 600, 0, SLAB_Z1),
    ("Car", "covered_car", (CX0 + CX1) / 2, (CY0 + CY1) / 2, 180, 20),
]
for name, asset, x, y, r, z in FURNITURE:
    vc.append(["planting.add_single", {"name": name, "asset": asset, "position": w(x, y, z),
                                       "rotation_y_deg": r}])
# Pendants over the island and the table (fittings that light themselves), hung from the ceiling.
for name, x, y in (("Island pendant", (IX0 + IX1) / 2, (IY0 + IY1) / 2), ("Dining pendant", 12100, LIV_Y - 300)):
    vc.append(["planting.add_single", {"name": name, "asset": "modern_ceiling_lamp_01",
                                       "position": w(x, y, CEIL_Z - 952), "rotation_y_deg": 0}])

# Recessed downlights, 600 lm warm white, 300 mm under the ceiling.
DOWN = [("Living", x, LIV_Y) for x in (2000, 4800)] + [("Kitchen", x, YC - 1200) for x in (7000, 9800)]
DOWN += [("Hall", (XA + XB) / 2, YC + 1800), ("Corridor", XB + 2500, YD + 680), ("Corridor", XB + 6500, YD + 680)]
DOWN += [("Master", XB + 1800, YD / 2), ("Master", XG - 1800, YD / 2), ("Ensuite", (XG + IX) / 2, YD / 2)]
DOWN += [("Den", 2300, YC + 1800), ("Utility", (X_DEN + X_UTIL) / 2, YC + 1800),
         ("Guest WC", (X_UTIL + XA) / 2, YC + 1800)]
DOWN += [("Bedroom 2", (XB + X_BED2) / 2, YC + 2000), ("Bathroom", (X_BED2 + X_BATH) / 2, YC + 1800),
         ("Bedroom 3", (X_BATH + IX) / 2, YC + 2000)]
SPOT = {"type": "spot", "direction": [0, -1, 0], "inner_cone_deg": 35, "outer_cone_deg": 70}
for name, x, y in DOWN:
    vc.append(["light.add", {"name": f"{name} downlight", "position": w(x, y, CEIL_Z - 300), "lumens": 600,
                             "color_temperature_k": 2700, "range_m": 8, "kind": SPOT}])
for k, x in enumerate(range(2000, W, 4000)):   # soffit lights along the covered terrace
    vc.append(["light.add", {"name": f"Soffit light {k + 1}", "position": w(x, -1500, WALL_TOP - 50),
                             "lumens": 400, "color_temperature_k": 2700, "range_m": 8, "kind": SPOT}])
vc.append(["light.add", {"name": "Front door light", "position": w((FD_A + FD_B) / 2, D + 900, WALL_TOP - 50),
                         "lumens": 400, "color_temperature_k": 2700, "range_m": 8}])

# Malibu, midsummer afternoon; the dusk view keeps its own hour.
vc.append(["site.set", {"latitude": 34.03, "longitude": -118.78, "utc_offset_hours": -7}])
vc.append(["time.set", {"month": 6, "day": 21, "hour": 16.0}])
VIEWS = [  # name, eye (X, Y, Z mm), target, vertical field of view, hour or None
    ("Sea side", (11000, -21000, 2600), (11000, 3000, 2200), 50, None),
    ("From the beach", (2000, -17000, 1200), (10000, 3000, 2200), 55, None),
    ("Entrance", (17000, 26000, 2200), (12000, 10000, 1800), 50, None),
    ("East", (44000, 2000, 3500), (12000, 5000, 1800), 45, None),
    ("West", (-24000, 7000, 4000), (8000, 4000, 1800), 45, None),
    ("Through the glass", (7500, -4200, FFL + 1500), (4000, 4500, FFL + 1100), 60, None),
    ("Living room", (13600, 1200, FFL + 1550), (1000, 4000, FFL + 1100), 65, None),
    ("Kitchen", (4600, 1300, FFL + 1550), (9500, 6000, FFL + 900), 65, None),
    ("Master bedroom", (XB + 600, YD - 1200, FFL + 1550), (XG, -1500, FFL + 900), 65, None),
    ("Dusk", (16000, -24000, 2500), (11000, 4000, 2000), 50, 20.3),
]
for name, eye, target, fov, hour in VIEWS:
    v = {"name": name, "position": w(*eye), "target": w(*target), "fov_y_deg": fov}
    if hour is not None:
        v["time"] = {"hour": hour}
    vc.append(["viewpoint.add", v])
with open(os.path.join(ROOT, "crates", "visioncraft-core", "src", "examples", "beach-house.json"), "w") as f:
    f.write("[\n" + ",\n".join("  " + json.dumps(s) for s in vc) + "\n]\n")

with open(os.path.join(here, "beach-house.steps.json"), "w") as f:
    f.write("[\n" + ",\n".join(json.dumps(s) for s in steps) + "\n]\n")
with open(os.path.join(here, "bodies.json"), "w") as f:
    json.dump({n: ids[n] for n in FINISHED}, f, indent=1)
    f.write("\n")
if "--rooms" in sys.argv:
    for room, (label, (x0, y0, x1, y1)) in ROOMS.items():
        print(f"| {label} | {(x1 - x0) / 1000:.2f} x {(y1 - y0) / 1000:.2f} | {(x1 - x0) * (y1 - y0) / 1e6:.1f} |")
print(f"{len(steps)} steps, {next_id[0] - 1} features, {len(FINISHED)} bodies", file=sys.stderr)
