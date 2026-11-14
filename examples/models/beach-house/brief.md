# Beach house: design brief

A single-storey California-style beach house on a low bluff, modelled for presentation renders:
a long flat-roofed bar with a deep overhang, a glass wall to the sea along the whole day wing,
a breezeway hall that looks through the house to the water, and a night wing behind a door.
All numbers are in millimetres unless stated; they are the parameters and plan lines of
`generate.py`, which is the source of truth.

## Site and orientation

- The sea is to the **south** (3DCraft -Y; VisionCraft +Z). The street is to the north.
- Origin: south-west outer corner of the house at ground level. Ground (Z 0) is not modelled.
- The house stands on a concrete plinth set back 150 so it reads as floating; the floor is
  600 above the ground, so there are steps down to the beach and up from the entrance path.

## Main dimensions

| Item | Value |
|---|---|
| Footprint, outside the cladding | 24,000 x 10,500 (252 m2 gross) |
| Plinth | set back 150, Z -300 to 400 |
| Floor slab | Z 400 to 580, 180 thick, flush with the facade |
| Finished floor level (FFL) | Z 600 (20 mm finishes on the slab) |
| Ceiling height | 2,850 (ceiling at Z 3,450, boards 20) |
| Outer walls | 300: 240 core (plastered inside) + 60 timber cladding |
| Inner walls | 120 |
| Roof | flat, 300 deep slab, Z 3,470 to 3,770, 20 membrane |
| Overhang | 3,000 on the sea side (covered terrace), 900 elsewhere |
| Fascia | 60 dark metal band, Z 3,420 to 3,890: 100 upstand above the membrane |
| Internal doors | 910 openings, 2,100 high, 850 x 40 leaves (shown open 90 degrees) |
| Front door | 1,600 opening, 2,400 high: 1,100 pivot leaf and a glazed sidelight |
| Windows | dark metal frames 60 x 80, set 60 behind the cladding face; 10 mm glass; lites at most 1,400 wide; external metal sills 50 proud |
| Living glass wall | 13,900 wide, floor to ceiling, 6 sliding bays on two tracks (one slid open) |
| Master glass wall | 5,200 wide, 2 sliding bays |
| Deck | 25,200 x 6,600, top Z 580 (20 below FFL), 45 boards ~142 wide running east-west |
| Steps to the beach | 4,800 wide, 4 risers of 145, goings 350 |
| Balustrade | frameless 12 mm glass, 1,100 above the deck, in a 100 high base shoe |
| Carport | 6,000 x 6,000 on four 120 steel posts, roof Z 2,700 to 2,950 |
| Entrance | landing 3,100 x 1,800 at slab level, 3 steps of 145 down to the path |

## Room schedule

Clear inside dimensions (metres) between finished wall faces.

| Room | Size (m) | Area (m2) | Floor | Notes |
|---|---|---|---|---|
| Living, dining, kitchen | 14.20 x 6.30 | 89.5 | oak boards | fireplace on the west wall, island, counter run and tall units on the north wall, the hall's gallery |
| Entrance hall | 2.38 x 3.60 | 8.6 | stone | coat closet, view south through the glass wall |
| Den | 4.10 x 3.48 | 14.3 | oak boards | built-in shelving |
| Utility and pantry | 5.48 x 3.48 | 19.1 | tiles | counter run, larder |
| Guest WC | 1.88 x 3.48 | 6.5 | tiles | WC, vanity, mirror |
| Corridor (night wing) | 9.08 x 1.20 | 10.9 | oak boards | linen cupboard at the end |
| Master bedroom | 5.78 x 4.98 | 28.8 | oak boards | sliding glass wall to the deck, wall of wardrobes |
| Ensuite | 3.18 x 4.98 | 15.8 | tiles | bath under the window, double vanity, walk-in shower, WC |
| Bedroom 2 | 3.78 x 3.48 | 13.2 | oak boards | wardrobe |
| Bathroom | 2.08 x 3.48 | 7.2 | tiles | bath, vanity, WC |
| Bedroom 3 | 2.98 x 3.48 | 10.4 | oak boards | wardrobe |
| **Total net** | | **224.3** | | |

## Plan

One character is about 400 mm across, one line about 750 mm. `d` is a door, `=` glass.

```
                                 N  (street)          path to the street
                                                            ||
 +----------+-------------+-----+ +---===[door]--+----------+------+--------+
 | DEN      | UTILITY /   | WC  d |   HALL       | BEDROOM 2| BATH | BED 3  |
 | 14.3     | PANTRY 19.1 | 6.5 | |   8.6        | 13.2     | 7.2  | 10.4   |
 |      [sh]|[larder][counter]  | | [coats]      |[wardrobe]|      |[ward.] |
 +------d---+--d----------+-----+ +              +---d------+--d---+---d----+
 |                [counter run][tall]            d   CORRIDOR 10.9   [linen]|
 |[fire]                                         +-d-------------+-+--------+
 |[bench]    LIVING / DINING / KITCHEN           | MASTER  28.8  | | ENSUITE|
 |           89.5         [ island ]             | [wardrobes]   d | 15.8   |
 |                                               |               | | [bath] |
 +=====================sliding, 6 bays===========+==sliding 2====+-+--==---+
 :  covered terrace under the 3.0 m overhang                               :
 :[outdoor                                                                 :
 : kitchen]       DECK 25.2 x 6.6, glass balustrade          [fire pit]    :
 +.............[  steps to the beach  ]....................................+
                                 S  (sea)
```

West of the north rooms, detached by a gap of 600: the carport, with a path of pavers along
the north facade to the entrance landing.

## Element groups (one 3DCraft body each)

`walls-ext`, `cladding+` (with the carport's timber screen), `walls-int`, `plinth`, `slab`,
`roof`, `roof-membrane`, `fascia`, `columns`, `ceiling`, `ceiling-living`, `glass` (every
pane, each its own closed shell: window lites, sliding panels, sidelight, shower screen,
balustrade), `frames`, `sills`, `doors`, `door-frames`, `hardware`, `floor-<room>` (11),
`fireplace`, `firebox`, `kitchen-units`, `worktops`, `appliances`, `joinery`, `sanitary`,
`baths`, `sanitary-ledge`, `mirrors`, `deck`, `deck-base`, `steps`, `railing`, `firepit`,
`carport-pad`, `entrance`, `path`.

## Left out on purpose

Terrain, plants, sea and loose furniture (VisionCraft adds them); door handles except the
front door's pull; sink taps, light fittings; deck joists (a dark base block stands under the
boards); gutters and downpipes.
