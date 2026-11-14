"""Writes crates/visioncraft-core/src/examples/coast.json, the `coast` example's own part."""
import json
import os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..")

C = []


def c(cmd, **p):
    C.append([cmd, p])


def pal(*ids):
    return [{"asset": a if ":" in a or a.startswith(("crystalline", "flower", "cheiri", "coast")) else "flora:" + a, "weight": w}
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
# Mainland: 1 km of coast, 600 m deep; the land rises inland and towards both ends to 6 m above
# the sea, the height of the endless ground beyond it.
c("terrain.create", origin=[-500, -320], width_m=1000, depth_m=600, cell_m=3.0, base_material="lawn")
for pts, r in [([[-500, -300], [500, -300]], 150), ([[-500, -200], [-500, 40]], 130), ([[500, -200], [500, -60]], 130)]:
    c("terrain.stroke", tool="flatten", points=pts, radius_m=r, strength=1.0, falloff=0.9, target_height=6 + LEVEL)
# Low dune ridges behind the house.
c("terrain.stroke", tool="raise", points=[[-320, -80], [-40, -70]], radius_m=40, strength=0.3, falloff=0.9)
c("terrain.stroke", tool="raise", points=[[60, -72], [330, -95]], radius_m=40, strength=0.3, falloff=0.9)
c("terrain.stroke", tool="raise", points=[[-200, -150], [220, -160]], radius_m=55, strength=0.35, falloff=0.9)
# The house plot level with the floor (1.8 m above the sea), sloping off into the dunes.
c("terrain.stroke", tool="flatten", points=[[-40, -45], [55, -45], [55, 4], [-40, 4], [-40, -45]], radius_m=18, strength=1.0, falloff=0.6, target_height=0)
# The coast: a gentle bay in front of the house, bluffs towards both ends and the headland's root
# in the west; from the house the beach runs 25 m to the waterline.
c("water.set_sea", level=LEVEL, kind="sea", sea_state=3.2, direction_deg=355, clarity=0.8, foam=0.55,
  coast=[[-4000, -300], [-2700, 150], [-2150, 1250], [-1850, 1350], [-1550, 850], [-1000, 330], [-500, 190],
         [-340, 85], [-230, 40], [-110, 35], [110, 33], [230, 20], [360, -3], [500, -27], [1500, -160], [4000, -450]])
c("terrain.shore", points=[[-500, 150], [-430, 120], [-340, 45], [-230, 7], [-110, 0], [0, 0], [110, -2], [230, -13], [360, -38], [500, -62]],
  top_m=6, bank_m=15, berm_m=1.0, beach_m=20, shelf_m=250, depth_m=6, material="sand")
# Sand, not grass, between the house and the beach (layer 1 is the shore's sand).
c("terrain.stroke", tool="paint", points=[[-62, 9.5], [82, 9.5]], radius_m=6, strength=1.0, falloff=0.4, layer=1)
c("terrain.stroke", tool="paint", points=[[-8, 4.5], [27, 4.5]], radius_m=4, strength=1.0, falloff=0.4, layer=1)
c("view.set", ground={"height": 6 + LEVEL, "color": [0.1, 0.13, 0.05]}, wind_direction_deg=0, wind_strength=0.5, haze=1.4)

# Hard landscape: the drive to the carport, the entrance court, the garden room's lawn, paths.
c("shape.add_area", name="Drive", outline=[[-7.5, -11.8], [-0.8, -11.8], [-0.8, -31], [-1.5, -75], [-8.5, -75], [-8, -31]], material="gravel")
c("shape.add_area", name="Entrance court", outline=rect(-0.8, -19.5, 20, -31), material="paving")
c("shape.add_area", name="Garden room lawn", outline=rect(30, -17, 43.5, 0), material="lawn")
c("shape.add_area", name="Garden path", outline=[[25, 3.2], [30, -0.5], [30, 1.2], [25.6, 4.8]], material="gravel")
c("shape.add_area", name="Beach walk", outline=rect(6.8, 7.65, 8.8, 15), material="timber")
c("shape.add_area", name="West gravel garden", outline=[[-34, -22], [-9, -22], [-9, 2], [-34, 2]], material="gravel")

# Wind shelter: hedges round the garden room on the land side and the east.
c("planting.add_spline", name="Garden room hedge", asset_palette=pal(("ligustrum-hedge", 1)),
  points=[[27, -21.5], [47.5, -21.5], [47.5, 3]], straight=True, spacing={"every": 0.8})
c("planting.add_spline", name="Court hedge", asset_palette=pal(("laurocerasus-hedge", 1)),
  points=[[20.8, -19.5], [20.8, -35.5], [0.5, -35.5]], straight=True, spacing={"every": 0.8})

# Trees and large shrubs (the sea side stays open in front of the glass wall, x 0..21).
c("planting.add_spline", name="Drive cypresses west", asset_palette=pal(("cupressus-sempervirens", 1)), points=[[-11, -36], [-11.5, -72]], straight=True, spacing={"count": 5})
c("planting.add_spline", name="Drive cypresses east", asset_palette=pal(("cupressus-sempervirens", 1)), points=[[2.5, -36], [2.0, -72]], straight=True, spacing={"count": 5})
for name, sp, pos, s in [
    ("Court olive", "olea-europaea", [9.5, 0, -25.5], 1.0),
    ("Olive by the hedge", "olea-europaea", [26.0, 0, -28.0], 1.0),
    ("Olive west", "olea-europaea", [-20.0, 0, -16.0], 1.0),
    ("Fig in the garden room", "ficus-carica", [33.0, 0, -13.5], 1.0),
    ("Canary palm east", "phoenix-canariensis", [44.0, 0, 4.0], 1.0),
    ("Canary palm west", "phoenix-canariensis", [-13.0, 0, 5.0], 1.0),
    ("Fan palm west 1", "trachycarpus-fortunei", [-18.5, 0, 0.5], 1.1),
    ("Fan palm west 2", "trachycarpus-fortunei", [-22.0, 0, -4.0], 0.9),
    ("Fan palm west 3", "trachycarpus-fortunei", [-16.0, 0, -6.5], 1.0),
    ("Fan palm east", "trachycarpus-fortunei", [36.5, 0, 5.0], 1.0),
    ("Pine west", "pinus-sylvestris", [-40.0, 0, -30.0], 1.0),
    ("Pine east", "pinus-sylvestris", [56.0, 0, -32.0], 1.0),
]:
    c("planting.add_single", name=name, asset=f"flora:{sp}", position=pos, scale=s)
c("planting.add_spline", name="Laurel standards", asset_palette=pal(("laurus-standard", 1)), points=[[1.2, -32.5], [19.0, -32.5]], straight=True, spacing={"count": 4})
c("planting.add_cluster", name="Mountain pines west", asset_palette=pal(("pinus-mugo", 1)), center=[-27, -12], radius_m=6.5, count=3, seed=31)
c("planting.add_cluster", name="Pampas grass west", asset_palette=pal(("cortaderia-selloana", 1)), center=[-30, -3], radius_m=2.5, count=3, seed=32)
c("planting.add_spline", name="Garden room shrubs", asset_palette=pal(("rosa-shrub", 2), ("hibiscus-syriacus", 1), ("photinia-red-robin", 1)),
  points=[[28.5, -20.2], [46.2, -20.2], [46.2, -1]], straight=True, spacing={"every": 2.4}, seed=33)
c("planting.add_spline", name="Court shrubs", asset_palette=pal(("photinia-red-robin", 1), ("buxus-ball", 1)),
  points=[[1.5, -34.3], [19.5, -34.3]], straight=True, spacing={"every": 2.0}, seed=34)

# Borders in layers and drifts: tall at the back, low in front, each species massed and repeated.
drifts("Garden room north mid", 28.5, 44, -19.2, -17.9, ["lavandula-angustifolia", "echinacea-purpurea", "salvia-nemorosa", "rudbeckia-fulgida", "lavandula-angustifolia"], [2.2, 3.0, 3.0, 3.0, 2.2], 40)
drifts("Garden room north front", 28.5, 44, -17.9, -17.2, ["thymus-serpyllum", "festuca-glauca", "geranium-rozanne", "festuca-glauca"], [3.0, 3.0, 2.5, 3.0], 50)
drifts("Garden room east mid", 44.2, 45.4, -18, 0, ["nepeta-faassenii", "echinacea-purpurea", "lavandula-angustifolia", "salvia-nemorosa"], [3.0, 3.0, 2.2, 3.0], 60, along="z")
drifts("Garden room east front", 43.6, 44.2, -18, 0, ["festuca-glauca", "sedum-herbstfreude", "festuca-glauca"], [3.0, 2.5, 3.0], 70, along="z")
drifts("Court border", 1.5, 19.5, -33.3, -31.3, ["lavandula-angustifolia", "pennisetum-alopecuroides", "salvia-nemorosa", "lavandula-angustifolia", "iris-germanica", "pennisetum-alopecuroides"], [2.2, 1.6, 3.0, 2.2, 3.0, 1.6], 80)
drifts("Drive verge west", -10, -8.8, -70, -32, ["lavandula-angustifolia", "festuca-glauca", "pennisetum-alopecuroides", "lavandula-angustifolia", "festuca-glauca"], [1.6, 2.0, 1.2, 1.6, 2.0], 90, along="z")
drifts("Drive verge east", -0.4, 1.0, -70, -36, ["festuca-glauca", "lavandula-angustifolia", "pennisetum-alopecuroides", "festuca-glauca", "lavandula-angustifolia"], [2.0, 1.6, 1.2, 2.0, 1.6], 100, along="z")
# The west gravel garden: Mediterranean and South African coastal plants in loose drifts.
area("Gravel garden lavender", [[-33, -21], [-24, -21], [-26, -14], [-33, -15]], [("lavandula-angustifolia", 1)], 1.4, 110)
area("Gravel garden gazania", [[-14, -6], [-9.5, -6], [-9.5, 1.5], [-13, 1.5]], [("flower_gazania", 1)], 3.0, 111)
area("Gravel garden grasses", [[-24, -21], [-10, -21], [-10, -12], [-22, -13]], [("pennisetum-alopecuroides", 2), ("festuca-glauca", 3)], 1.3, 112)
area("Gravel garden succulents", [[-33, -9], [-25, -9], [-24, 1.5], [-33, 1.5]], [("cheiridopsis_succulent", 1), ("crystalline_iceplant", 1)], 1.6, 113)
area("Gravel garden salvia", [[-24, -11], [-12, -11], [-12, -7], [-24, -8]], [("salvia-nemorosa", 1)], 2.5, 114)
area("Gravel garden ursinia", [[-24, -2], [-15, -2], [-15, 1.5], [-24, 1.5]], [("flower_ursinia", 1)], 2.5, 115)

# Towards the beach: dune grass on the low dune either side of the walk, ice plant on its face.
area("Dune grass west", [[-60, 8], [6.2, 8], [6.2, 12.5], [-60, 13.5]], [("festuca-glauca", 3), ("pennisetum-alopecuroides", 2), ("calamagrostis-acutiflora", 1)], 0.7, 120)
area("Dune grass east", [[9.4, 8], [80, 8], [80, 12.5], [9.4, 12.5]], [("festuca-glauca", 3), ("pennisetum-alopecuroides", 2), ("calamagrostis-acutiflora", 1)], 0.7, 121)
area("Ice plant on the dune", [[-60, 12.5], [6.2, 12.5], [6.2, 14.5], [-60, 15]], [("crystalline_iceplant", 1)], 0.8, 122)
area("Ice plant east", [[9.4, 12.5], [80, 12.5], [80, 14.5], [9.4, 14.5]], [("crystalline_iceplant", 1)], 0.8, 123)

# The coastal scrub over the dunes inland, thinning out with pines.
area("Inland pines", [[-160, -45], [160, -45], [160, -170], [-160, -170]], [("pinus-sylvestris", 2), ("olea-europaea", 1)], 0.0008, 135, avoid_buildings=True)
area("Coastal scrub", [[-160, -45], [-42, -45], [-42, -80], [60, -80], [60, -45], [160, -45], [160, -170], [-160, -170]],
     [("cortaderia-selloana", 1), ("festuca-glauca", 4), ("lavandula-angustifolia", 2)], 0.012, 131)

# Pots on the deck, a bench in the garden room, lights along the drive and the garden path.
c("planting.add_single", name="Deck pot west", asset="flora:laurus-standard", position=[0.8, 0.58, 5.9], scale=0.7)
c("planting.add_single", name="Deck pot east", asset="flora:laurus-standard", position=[24.2, 0.58, 5.9], scale=0.7)
c("planting.add_single", name="Garden bench", asset="painted_wooden_bench", position=[37.0, 0, -16.2])
c("planting.add_spline", name="Drive lights", asset_palette=pal(("vc-path-light", 1)), points=[[-9.2, -14], [-9.6, -72]], straight=True, spacing={"count": 7})
c("planting.add_spline", name="Garden path lights", asset_palette=pal(("vc-path-light", 1)), points=[[26, 5.6], [30.6, 2]], straight=True, spacing={"count": 2})

V = [
    ("Overview", [70, 30, 95], [10, -2, 0], 50, {}),
    ("From the beach", [38, -0.2, 40], [8, 1.0, 2], 55, {}),
    ("Deck over the sea", [12, 2.3, 5], [6, 0.2, 60], 65, {}),
    ("Living room to the sea", [8, 2.2, -6], [7, 1.1, 30], 65, {}),
    ("Drone along the coast", [-120, 45, 120], [80, -2, 10], 50, {}),
    ("Golden hour, the house and the sun on the water", [60, 0.0, 30], [-40, 1.0, 30], 60, {"time": {"month": 12, "day": 15, "hour": 17.25}}),
    ("Dusk, the house lit", [22, 0.0, 28], [11, 1.8, 0], 60, {"time": {"month": 7, "day": 15, "hour": 20.45}}),
    ("From the water", [18, -1.3, 75], [10, 1.5, 0], 45, {}),
    ("Surf at the waterline", [30, -0.15, 31], [0, -1.8, 46], 60, {}),
    ("Entrance court", [1.5, 1.7, -30], [14, 1.3, -14], 60, {}),
    ("Garden room", [31, 1.7, -2], [42, 0.8, -19], 65, {}),
    ("Border in flower", [36, 1.1, -14.5], [37, 0.3, -19], 60, {}),
    ("The garden in winter", [56, 4, -40], [20, 0, -10], 55, {"season": "winter"}),
    ("The garden after 10 years", [56, 4, -40], [20, 0, -10], 55, {"years": 10}),
    ("Plan", [10, 140, -10.01], [10, 0, -10], 50, {}),
]
for name, pos, tgt, fov, extra in V:
    c("viewpoint.add", name=name, position=pos, target=tgt, fov_y_deg=fov, **extra)

with open(os.path.join(ROOT, "crates", "visioncraft-core", "src", "examples", "coast.json"), "w") as f:
    f.write("[\n" + ",\n".join("  " + json.dumps(x) for x in C) + "\n]\n")
print(len(C), "commands")
