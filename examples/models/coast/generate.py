"""Writes crates/visioncraft-core/src/examples/coast.json, the `coast` example's own part."""
import json
import math
import os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..")

C = []


def c(cmd, **p):
    C.append([cmd, p])


def pal(*ids):
    # Catalogue models have underscores in their ids, the procedural species hyphens.
    return [{"asset": a if ":" in a or "_" in a else "flora:" + a, "weight": w}
            for a, w in ids]


def area(name, outline, palette, density, seed, **kw):
    c("planting.add_area", name=name, asset_palette=pal(*palette), outline=outline,
      density_per_m2=density, seed=seed, **kw)


def rect(x0, z0, x1, z1):
    return [[x0, z0], [x1, z0], [x1, z1], [x0, z1]]


def drifts(prefix, x0, x1, z0, z1, species, density, seed, along="x"):
    """A border cut into drifts of one species each, repeated along its length."""
    n = len(species)
    for i, (sp, d) in enumerate(zip(species, density)):
        if along == "x":
            a, b = x0 + (x1 - x0) * i / n, x0 + (x1 - x0) * (i + 1) / n
            o = rect(round(a, 2), z0, round(b, 2), z1)
        else:
            a, b = z0 + (z1 - z0) * i / n, z0 + (z1 - z0) * (i + 1) / n
            o = rect(x0, round(a, 2), x1, round(b, 2))
        area(f"{prefix} {i + 1}", o, [(sp, 1)], d, seed + i)


LEVEL = -1.8
c("site.set", latitude=34.03, longitude=-118.78, utc_offset_hours=-7)
c("time.set", year=2026, month=7, day=15, hour=16.5)
# Mainland: 1 km of coast, 600 m deep, sand and coastal scrub (no lawn but the garden room's); the
# land rises inland and towards both ends to 6 m above the sea, the height of the endless ground.
c("terrain.create", origin=[-500, -320], width_m=1000, depth_m=600, cell_m=3.0, base_material="sand")
for pts, r in [([[-500, -300], [500, -300]], 150), ([[-500, -200], [-500, 40]], 130), ([[500, -200], [500, -60]], 130)]:
    c("terrain.stroke", tool="flatten", points=pts, radius_m=r, strength=1.0, falloff=0.9, target_height=6 + LEVEL)
# Dune ridges behind the house and smaller dunes over them (raised only: a hollow below the
# sea's level would fill with sea water).
c("terrain.stroke", tool="raise", points=[[-320, -80], [-40, -70]], radius_m=40, strength=0.3, falloff=0.9)
c("terrain.stroke", tool="raise", points=[[60, -72], [330, -95]], radius_m=40, strength=0.3, falloff=0.9)
c("terrain.stroke", tool="raise", points=[[-200, -150], [220, -160]], radius_m=55, strength=0.35, falloff=0.9)
for pts, r, k in [([[-150, -105], [-60, -120]], 18, 0.5), ([[-20, -95], [40, -110]], 15, 0.6), ([[90, -120], [170, -105]], 20, 0.5),
                  ([[-110, -60], [-70, -75]], 12, 0.4), ([[70, -60], [120, -70]], 12, 0.4), ([[-60, -185], [60, -195]], 25, 0.4)]:
    c("terrain.stroke", tool="raise", points=pts, radius_m=r, strength=k, falloff=0.8)
# The house plot level with the floor (1.8 m above the sea), sloping off into the dunes.
c("terrain.stroke", tool="flatten", points=[[-40, -45], [55, -45], [55, 4], [-40, 4], [-40, -45]], radius_m=18, strength=1.0, falloff=0.6, target_height=0)
# The coast: a gentle bay in front of the house, bluffs towards both ends and the headland's root
# in the west. In front of the house a low bank (z 10 to 11.5; the grid has 3 m cells) and 15.5 m of beach: the waterline lies
# about 20 m beyond the deck's edge (z 6.6), the swash's run-up (R2 about 0.25 m) about 15 m.
c("water.set_sea", level=LEVEL, kind="sea", sea_state=3.2, direction_deg=355, clarity=0.8, foam=0.55,
  coast=[[-4000, -300], [-2700, 150], [-2150, 1250], [-1850, 1350], [-1550, 850], [-1000, 330], [-500, 190],
         [-340, 85], [-230, 35], [-110, 27], [110, 25], [230, 12], [360, -11], [500, -35], [1500, -160], [4000, -450]])
c("terrain.shore", points=[[-500, 150], [-430, 120], [-340, 45], [-230, 10], [-110, 10], [0, 10], [110, 8], [230, -8], [360, -36], [500, -62]],
  top_m=6, bank_m=1.5, berm_m=1.0, beach_m=15.5, shelf_m=250, depth_m=6, material="sand")
# Offshore the bed falls away towards the endless ground's sea floor (20 m under the surface), so
# the terrain does not end in an underwater step: the surf over a 6 m shelf stopped at its edge,
# a line across the sea seen from the air.
for pts in ([[-520, 300], [520, 300]], [[525, 80], [525, 300]], [[-525, 270], [-525, 300]]):
    c("terrain.stroke", tool="flatten", points=pts, radius_m=110, strength=1.0, falloff=0.9, target_height=LEVEL - 20)
c("view.set", ground={"height": 6 + LEVEL, "color": [0.2, 0.18, 0.11]}, wind_direction_deg=0, wind_strength=0.5, haze=1.4, surroundings="dunes")

# ---- Hard landscape --------------------------------------------------------------------------
# House: x -0.6..24.6, z -10.5..0, deck to z 6.6; carport x -6.6..-0.6, z -10.5..-4.5; the
# front door at x 13.3 on the north face, its landing to z -12.3.
DOOR_X = 13.3
c("shape.add_area", name="Drive", outline=[[-7.5, -11.8], [-0.8, -11.8], [-0.8, -31], [-1.5, -75], [-8.5, -75], [-8, -31]], material="gravel")
# The entrance court on the door's axis, 10 m wide, linked to the drive by a paved walk.
c("shape.add_area", name="Entrance court", outline=[[-0.8, -12.3], [DOOR_X + 5, -12.3], [DOOR_X + 5, -24], [DOOR_X - 5, -24],
                                                     [DOOR_X - 5, -14.8], [-0.8, -14.8]], material="paving")
# The garden room off the living side (west): a lawn panel framed by deep borders.
c("shape.add_area", name="Garden room lawn", outline=rect(-19.7, -4.6, -8.6, 1.5), material="lawn")
c("shape.add_area", name="Garden path", outline=rect(-21, 1.5, -1.2, 3.0), material="gravel")
c("shape.add_area", name="Beach walk", outline=rect(6.8, 7.65, 8.8, 14), material="timber")
# The gravel garden east of the night wing.
c("shape.add_area", name="East gravel garden", outline=rect(25.5, -16, 44, 4.5), material="gravel")
# The planted beds are dug and mulched soil, not the dune sand round them: between young or low
# plants a border shows its earth, which read as bare sand.
for name, o in [("Garden room north bed", rect(-23.8, -8.7, -9, -4.6)), ("Garden room west bed", rect(-23.8, -4.5, -19.7, 1.0)),
                ("Garden room south bed", rect(-21, 3.1, -6, 4.5)), ("Court west bed", rect(DOOR_X - 8.5, -24, DOOR_X - 5.1, -15.2)),
                ("Court east bed", rect(DOOR_X + 5.1, -24, DOOR_X + 8.5, -12.6)), ("Court olive bed", rect(DOOR_X - 4, -29.5, DOOR_X + 4, -25))]:
    c("shape.add_area", name=name, outline=o, material="soil")

# ---- Structure: hedges and trees -------------------------------------------------------------
# Evergreen yew round the garden room on the land side and the west (shelter, winter structure);
# open to the sea on the south.
c("planting.add_spline", name="Garden room hedge", asset_palette=pal(("taxus-hedge", 1)),
  points=[[-8.5, -9.2], [-24.6, -9.2], [-24.6, 5.0]], straight=True, spacing={"every": 0.7})
# The court closed on the north by a laurel hedge behind its focal olive.
c("planting.add_spline", name="Court hedge", asset_palette=pal(("laurocerasus-hedge", 1)),
  points=[[DOOR_X - 9, -31], [DOOR_X + 9, -31]], straight=True, spacing={"every": 0.8})
c("planting.add_spline", name="Drive cypresses west", asset_palette=pal(("cupressus-sempervirens", 1)), points=[[-11, -36], [-11.5, -72]], straight=True, spacing={"count": 5})
c("planting.add_spline", name="Drive cypresses east", asset_palette=pal(("cupressus-sempervirens", 1)), points=[[2.5, -36], [2.0, -72]], straight=True, spacing={"count": 5})
# Clipped bay standards in pairs along the court: a rhythm leading to the olive.
for side, x in (("west", DOOR_X - 4.2), ("east", DOOR_X + 4.2)):
    c("planting.add_spline", name=f"Court standards {side}", asset_palette=pal(("laurus-standard", 1)),
      points=[[x, -15.5], [x, -23]], straight=True, spacing={"count": 3})
for name, sp, pos, s in [
    # Focal points at the axes' ends and a canopy over the terrace: olives, repeated.
    ("Court olive", "olea-europaea", [DOOR_X, 0, -27.0], 1.1),
    ("Terrace olive", "olea-europaea", [-4.0, 0, 4.2], 1.0),
    ("Garden room olive", "olea-europaea", [-21.8, 0, -2.0], 0.9),
    # Well east of the house, in the gravel garden: nearer, their crowns showed over the roof
    # from the entrance court, looking like pots standing on it. From the court's north end the
    # sight line past the house's east end (x 24.6) must clear it with the trunk in view.
    ("Fan palm east 1", "trachycarpus-fortunei", [39.0, 0, 1.0], 1.1),
    ("Fan palm east 2", "trachycarpus-fortunei", [36.0, 0, -1.5], 0.9),
    ("Fan palm east 3", "trachycarpus-fortunei", [32.5, 0, -4.5], 1.0),
    ("Canary palm east", "phoenix-canariensis", [44.0, 0, 5.0], 1.0),
]:
    c("planting.add_single", name=name, asset=f"flora:{sp}", position=pos, scale=s)


# ---- Borders: 3 to 4 m deep, shrubs at the back, perennials and grasses in drifts, a low front ----
def drifts_in(prefix, o, species, seed):
    """Drifts of 7 to 15 plants of one species each, along the band o = (x0, z0, x1, z1)."""
    x0, z0, x1, z1 = o
    along_x = abs(x1 - x0) >= abs(z1 - z0)
    n = len(species)
    for i, (sp, d) in enumerate(species):
        if along_x:
            a, b = x0 + (x1 - x0) * i / n, x0 + (x1 - x0) * (i + 1) / n
            r = rect(round(min(a, b), 2), min(z0, z1), round(max(a, b), 2), max(z0, z1))
        else:
            a, b = z0 + (z1 - z0) * i / n, z0 + (z1 - z0) * (i + 1) / n
            r = rect(min(x0, x1), round(min(a, b), 2), max(x0, x1), round(max(a, b), 2))
        area(f"{prefix} drift {i + 1}", r, [(sp, 1)], d, seed * 10 + i, **BED)


# Planted beds: each plant near its own spot (a full cell of jitter left holes a plant wide
# between neighbours planted their mature spread apart) and neighbours a little different in size.
BED = dict(position_jitter=0.5, scale_min=0.85, scale_max=1.1)


def border(prefix, back_outline, mid_band, front_outline, back, mid, front, seed, front_density=4.0):
    area(f"{prefix} back", back_outline, back, 0.25, seed, **BED)
    drifts_in(prefix, mid_band, mid, seed + 1)
    area(f"{prefix} front", front_outline, front, front_density, seed + 2, **BED)


# A front of two blue fescues (0.3 m across) to one thyme or cranesbill (0.5 to 0.7 m), each
# planted its mature spread apart: (2 * 0.3^2 + 0.5^2) / 3 = 0.14 m2 a plant, about 7 per m2
# (at 4 the fescues stood as blue dots in sand).
FESCUE_FRONT = 7.0
MID = [("pennisetum-alopecuroides", 2.0), ("salvia-nemorosa", 4.0), ("echinacea-purpurea", 4.0),
       ("calamagrostis-acutiflora", 3.0), ("nepeta-faassenii", 3.5), ("lavandula-angustifolia", 3.0)]
# Garden room: the north border (z -8.7 to -4.6) and the west border (x -23.8 to -19.7).
border("Garden room north", rect(-23.8, -8.7, -9, -7.6), (-23.8, -7.6, -9, -5.4), rect(-23.8, -5.4, -9, -4.6),
       [("taxus-cone", 1), ("pinus-mugo", 1), ("buxus-sempervirens", 1)], MID,
       [("festuca-glauca", 2), ("thymus-serpyllum", 1)], 200, front_density=FESCUE_FRONT)
border("Garden room west", rect(-23.8, -4.5, -22.8, 1.0), (-22.8, -4.5, -20.6, 1.0), rect(-20.6, -4.5, -19.7, 1.0),
       [("taxus-cone", 1), ("pinus-mugo", 1)], MID[::-1][:4],
       [("festuca-glauca", 2), ("geranium-rozanne", 1)], 210, front_density=FESCUE_FRONT)
# The south edge of the room stays low, so the sea is in view from the lawn.
area("Garden room south edge", rect(-21, 3.1, -6, 4.5), [("lavandula-angustifolia", 2), ("festuca-glauca", 1)], 3.0, 220, **BED)
# Entrance court: borders either side of the standards, 3.4 m deep.
border("Court west", rect(DOOR_X - 8.5, -24, DOOR_X - 7.6, -15.2), (DOOR_X - 7.6, -24, DOOR_X - 5.8, -15.2),
       rect(DOOR_X - 5.8, -24, DOOR_X - 5.1, -15.2), [("taxus-cone", 1)], MID[:4], [("buxus-ball", 1)], 230)
border("Court east", rect(DOOR_X + 7.6, -24, DOOR_X + 8.5, -12.6), (DOOR_X + 5.8, -24, DOOR_X + 7.6, -12.6),
       rect(DOOR_X + 5.1, -24, DOOR_X + 5.8, -12.6), [("taxus-cone", 1)], MID[2:], [("buxus-ball", 1)], 240)
area("Court olive bed", rect(DOOR_X - 4, -29.5, DOOR_X + 4, -25), [("lavandula-angustifolia", 3), ("festuca-glauca", 2)], 2.5, 250, **BED)
drifts("Drive verge west", -10, -8.8, -70, -32, ["lavandula-angustifolia", "festuca-glauca", "pennisetum-alopecuroides", "lavandula-angustifolia", "festuca-glauca"], [1.6, 2.0, 1.2, 1.6, 2.0], 90, along="z")
drifts("Drive verge east", -0.4, 1.0, -70, -36, ["festuca-glauca", "lavandula-angustifolia", "pennisetum-alopecuroides", "festuca-glauca", "lavandula-angustifolia"], [2.0, 1.6, 1.2, 2.0, 1.6], 100, along="z")
# The east gravel garden: Mediterranean and South African coastal plants in loose drifts round
# a group of mountain pines and a boulder.
area("Gravel garden lavender", [[31, -15], [43, -15], [43, -9], [33, -10]], [("lavandula-angustifolia", 1)], 1.4, 110)
area("Gravel garden grasses", [[31, -8], [43, -8], [43, -2], [33, -2]], [("pennisetum-alopecuroides", 2), ("festuca-glauca", 3), ("calamagrostis-acutiflora", 1)], 1.3, 112)
area("Gravel garden succulents", [[33, -1], [43, -1], [43, 4], [33, 4]], [("cheiridopsis_succulent", 1), ("crystalline_iceplant", 1), ("flower_gazania", 1)], 1.6, 113)
area("Gravel garden salvia", rect(26, -15, 30, -7), [("salvia-nemorosa", 1), ("sedum-herbstfreude", 1)], 2.5, 114)
c("planting.add_cluster", name="Gravel garden pines", asset_palette=pal(("pinus-mugo", 1)), center=[38, -12], radius_m=4.5, count=3, seed=31)
c("planting.add_single", name="Gravel garden boulder", asset="namaqualand_boulder_04", position=[40.0, 0, -5.0], scale=0.8)

# ---- Towards the beach: marram-like dune grass on the bank, ice plant at its foot ----------------
for side, x0, x1, seed in (("west", -60, 6.2, 120), ("east", 9.4, 80, 121)):
    area(f"Dune grass {side}", [[x0, 7.5], [x1, 7.5], [x1, 10.5], [x0, 11]],
         [("calamagrostis-acutiflora", 3), ("festuca-glauca", 2), ("pennisetum-alopecuroides", 1)], 0.9, seed)
    area(f"Ice plant {side}", [[x0, 10.5], [x1, 10.5], [x1, 12], [x0, 12.5]], [("crystalline_iceplant", 1)], 0.8, seed + 2)

# ---- The dunes inland: tussock grass and low scrub with sand between, pines and olives in groups --
SCRUB = [("calamagrostis-acutiflora", 4), ("festuca-glauca", 4), ("pennisetum-alopecuroides", 1), ("lavandula-angustifolia", 1),
         ("pinus-mugo", 1), ("wild_rooibos_bush", 1), ("didelta_spinosa", 1), ("searsia_lucida", 1)]
area("Coastal scrub near", [[-60, -50], [75, -50], [75, 7], [56, 7], [56, -44], [-41, -44], [-41, 7], [-60, 7]], SCRUB, 0.35, 131, edge_fade_m=6)
area("Coastal scrub inland", rect(-200, -200, 200, -50), SCRUB, 0.1, 132, avoid_buildings=True, edge_fade_m=40)
for k, (cx, cz, n, r) in enumerate([(-55, -60, 5, 10), (-90, -95, 7, 14), (-30, -120, 4, 9), (40, -70, 3, 7), (85, -85, 6, 12),
                                     (130, -130, 7, 15), (-140, -150, 5, 12), (10, -165, 4, 10), (64, -22, 3, 6), (-50, -20, 3, 6)]):
    pal_k = (("pinus-sylvestris", 3), ("olea-europaea", 1)) if k % 3 else (("olea-europaea", 2), ("pinus-sylvestris", 1))
    # Room for the crowns at 20 years (pines about 7 m across, olives 5 m) inside each clump.
    c("planting.add_cluster", name=f"Inland pines and olives {k + 1}", asset_palette=pal(*pal_k), center=[cx, cz],
      radius_m=round(4.0 * n ** 0.5 + r * 0.5, 1), count=n, seed=300 + k)

# ---- Objects: pots on the deck, a bench and a pot at the garden room's axes, lights -------------
c("planting.add_single", name="Deck pot west", asset="flora:laurus-standard", position=[0.8, 0.58, 5.9], scale=0.7)
c("planting.add_single", name="Deck pot east", asset="flora:laurus-standard", position=[24.2, 0.58, 5.9], scale=0.7)
# A wooden bench with a back (1.16 x 0.89 x 0.5 m) at scale 1.2: 1.4 m long along the north
# border, facing the lawn (south).
c("planting.add_single", name="Garden bench", asset="painted_wooden_bench", position=[-15.0, 0, -5.0], scale=1.2)
# The clay pot is 0.27 x 0.22 x 0.26 m: at scale 2.4 a 0.64 m wide, 0.53 m high terracotta pot.
c("planting.add_single", name="Garden room pot", asset="planter_pot_clay", position=[-21.6, 0, -1.5], scale=2.4)
# A clipped bay rising from it, so the axis ends in a green sphere over the pot.
c("planting.add_single", name="Garden room pot bay", asset="flora:laurus-standard", position=[-21.6, 0.5, -1.5], scale=0.8)
c("planting.add_spline", name="Drive lights", asset_palette=[{"asset": "vc-path-light", "weight": 1}], points=[[-9.2, -14], [-9.6, -72]], straight=True, spacing={"count": 7})
c("planting.add_spline", name="Garden path lights", asset_palette=[{"asset": "vc-path-light", "weight": 1}], points=[[-18, 3.4], [-4, 3.4]], straight=True, spacing={"count": 3})

V = [
    ("Overview", [70, 30, 95], [10, -2, 0], 50, {}),
    ("From the beach", [34, 0.2, 21], [9, 1.2, -2], 55, {}),
    ("Deck over the sea", [12, 2.3, 5], [6, 0.2, 60], 65, {}),
    ("Living room to the sea", [8, 2.2, -6], [7, 1.1, 30], 65, {}),
    ("Drone along the coast", [-120, 45, 120], [80, -2, 10], 50, {}),
    ("Golden hour, the house and the sun on the water", [60, -0.2, 22], [-40, 1.0, 26], 60, {"time": {"month": 12, "day": 15, "hour": 17.25}}),
    ("Dusk, the house lit", [22, -0.2, 20], [11, 1.8, 0], 60, {"time": {"month": 7, "day": 15, "hour": 20.45}}),
    ("From the water", [18, 0.5, 70], [10, 1.2, 0], 45, {}),   # from a boat deck, 2.3 m above the sea
    ("Surf at the waterline", [30, -0.3, 23], [0, -1.8, 40], 60, {}),
    ("Entrance court", [DOOR_X, 2.0, -13.2], [DOOR_X, 1.0, -28], 60, {}),
    ("Garden room", [-2.5, 2.1, 1.0], [-21.5, 0.5, -2.5], 60, {}),
    ("Border in flower", [-12, 1.5, -1.0], [-15.5, 0.4, -6.5], 60, {}),
    ("The garden in winter", [-23, 2.2, -8.4], [-4, 0.5, 4], 60, {"season": "winter"}),
    ("The garden after 10 years", [-23, 2.2, -8.4], [-4, 0.5, 4], 60, {"years": 10}),
    ("Plan", [10, 140, -10.01], [10, 0, -10], 50, {}),
]

for name, pos, tgt, fov, extra in V:
    c("viewpoint.add", name=name, position=pos, target=tgt, fov_y_deg=fov, **extra)

# ---- Films: four ready-made films for the Films panel (ids f1 to f4) -------------------------------
# Each shot sets its own time and weather, so the project's view settings stay as they are.
def film(name, shots):
    fid = "f%d" % (sum(1 for cmd, _ in C if cmd == "film.add") + 1)
    c("film.add", name=name, fps=25)
    for shot in shots:
        c("film.add_shot", id=fid, shot=shot)


# Down the drive, over the entrance court and up to the front door (13.3, -12.3), in the low
# afternoon sun; the walk is routed round the court beds and hedges.
film("Arrival", [
    {"kind": "walk", "route": [[-5, -56], [-4.6, -30], [-1, -20], [1, -13.6], [9.5, -13.5], [13.3, -15.5]],
     "duration_s": 17, "eye_height_m": 1.6, "look_at": [DOOR_X, 1.3, -12.3],
     "date": {"month": 7, "day": 15}, "time_of_day": [18.4, 18.9]}])

# From high over the sea, 128 m out, down to the house, then a third of a circle round it at 45 m
# and 22 m above its floor (the dunes behind rise to 6 m).
HOUSE = [12.5, 1.0, -5]
ORBIT_R, ORBIT_H, ORBIT_FROM = 45, 22, 255
orbit_start = [round(HOUSE[0] + ORBIT_R * math.cos(math.radians(ORBIT_FROM)), 2), HOUSE[1] + ORBIT_H,
               round(HOUSE[2] - ORBIT_R * math.sin(math.radians(ORBIT_FROM)), 2)]
film("Drone over the coast", [
    {"kind": "glide", "path": [[28, 40, 128], [16, 31, 82], orbit_start], "look_at": HOUSE, "duration_s": 8,
     "ease": "in"},
    {"kind": "orbit", "center": HOUSE, "radius_m": ORBIT_R, "height_m": ORBIT_H, "from_deg": ORBIT_FROM,
     "to_deg": ORBIT_FROM + 120, "duration_s": 9, "join": "continue", "ease": "out"}])

# Along the garden path past the lavender border, then over the lawn towards the garden room
# pot and its clipped bay; the route keeps clear of beds, hedges and trunks.
film("Garden walk", [
    {"kind": "walk", "route": [[-6, 2.3], [-14, 2.3], [-16, -0.5], [-16.8, -1.8]], "duration_s": 18,
     "eye_height_m": 1.6, "time_of_day": [15.5, 16.5]}])

# Along the beach, a few metres from the swash, looking at the house while the sun sets behind it
# into dusk (sunset 20.1 h on 15 July); a little mist and light shafts, the colour easing to warm.
film("Golden hour by the sea", [
    {"kind": "walk", "path": [[46, 23], [30, 23.5], [12, 23.5], [-4, 23]], "duration_s": 18,
     "eye_height_m": 1.6, "look_at": HOUSE, "time_of_day": [18.9, 19.8], "mist": [0.05, 0.15],
     "shafts": [0.3, 0.5], "grade": ["neutral", "warm"], "fov_y_deg": 55}])

with open(os.path.join(ROOT, "crates", "visioncraft-core", "src", "examples", "coast.json"), "w") as f:
    f.write("[\n" + ",\n".join("  " + json.dumps(x) for x in C) + "\n]\n")
print(len(C), "commands")
