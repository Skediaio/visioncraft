# Roadmap

What is planned, in rough order. Updated 2026-11-18. Items move up when they are picked up;
nothing here is a promise of a date.

## In progress

- **Denser borders:** `planting.add_border {outline | area, style: cottage | prairie | shade | dry,
  front | facing, height_range, species, closure, drift_m, seed}` plants a layered perennial
  border from a curated species list per style (all built-in species with growth data): short at
  the front, tall at the back (the front is given, or is the side away from the nearest wall,
  building or hedge), in elongated drifts of a few to a dozen plants with a low matrix species
  between them, spaced from each species' mature width (closure / w^2 per m2) and gap-filled, so a
  three-year-old border is closed (95 % of the ground under a crown) and a new one open (14 % at
  planting). An area scatter with a `border`; undoable, deterministic by seed; the garden
  example's border is made with it. Flowers already read from eye level and follow the season
  (checked at 5 to 17 m). Recipe 29. Next: the same layout along a path or a curved bed's
  centre line, a bloom calendar view (what flowers each month in the border), a swatch of
  neighbouring colours, bulbs for spring between the perennials, the coast example's borders
  moved onto it.
- **Weathering:** `material.update {weathering: {amount, dirt, moss, wear, streaks}}` and
  `view.set {weathering}` (one slider for the whole design, kept explicit: the view's `years` does
  not drive it) age standard materials with procedural dirt (near the ground, in crevices, under
  overhangs), moss (up-facing surfaces and the shaded side, low and damp, in patches), wear
  (lighter, smoother up-facing paving) and rain streaks down walls, in the renderer in world
  space, changing albedo and roughness only, at a few noise lookups and nothing when off. In the
  window: a Weathering section in Materials and an Age slider in Sun & sky. Next: texture-based
  moss and lichen from CC0 scans, leaf litter stains, efflorescence on brick, edge wear from
  real curvature, dirt measured from each object's own footing (on a hill today it follows the
  ground plane), weathering of terrain layers and lawns' edges, and ageing that follows the
  years of a design.
- **A plan as underlay:** `underlay.add` puts a PNG or JPEG (a floor plan, a site plan, a scan)
  flat under the drawing, unlit and see-through (`opacity`, `elevation`, its own `underlay`
  layer), casting no shadow and not picked; `underlay.calibrate` (two image points and the drawing
  points they land on) and `underlay.scale_by_distance` (a known dimension) set scale, turn and
  origin exactly. It travels in the bundle as `underlays/<name>`. In the window: Draw, Plan
  image. Next: PDF pages (a pure-Rust rasteriser; export as PNG meanwhile), more than one page,
  perspective-correcting a photographed plan, a plan that follows terrain instead of lying flat.
- **Photos on walls:** `wall_photo.add` puts a PNG or JPEG of a facade on one wall face as its
  texture: its four corners in the photo (`corners_px`, bottom-left first as seen from outside) are
  warped by a homography so the facade fills the face, or a typed `region` of it in metres (the
  rest of the face keeps its material); lit like any surface, shadows fall on it, a window's
  reveals keep the wall's material. `wall_photo.set` and `wall_photo.remove`; one photo a face; it
  travels in the bundle as `photos/<name>` and `document.inspect` lists it with the face's size. In
  the window: Materials, click a wall, Photo on wall…, drag four corner handles onto the facade,
  type a region, Apply. Next: several photos on one face (a facade in tiles), removing what
  stands in front of it (trees, cars), matching the photo's colour to the sun, EXIF rotation.
- **Films from camera moves:** `film.add` and `film.add_shot` describe a film as shots: `walk`
  (the eye follows the real ground, looks ahead), `glide` (a smooth curve through points), `rise`,
  `orbit` (round a spot, any number of turns), `flyover` (a drone looking ahead and down) and
  `hold` (a viewpoint or an eye and target), each with a duration or a speed, an ease for starts and
  stops, and a sun's course, date or season, years of growth and snow that change along the shot.
  `film.preview {id, t}` gives the camera and the changes at a time without drawing, so an agent
  checks its shots; `film.render` and `visioncraft-cli film` write numbered PNG frames (animation
  clock = film time, so wind and water move), and `--video` runs ffmpeg when it is installed. In the
  window: Views mode, Film panel (shots, add from the current camera, scrubber, play). Recipe 27.
  Next: smooth joins and fades between shots, a camera path drawn in the viewport, a spline
  walk, rendering frames from the window, a video without ffmpeg, a shot that follows a moving
  target.
- **Quantities:** `query.quantities` lists areas, lengths, volumes and plant counts per shape and
  planting, and totals per material and asset; with the host it also lists linked models per body
  (surface, volume of closed bodies, materials, scale applied) and counts plants with the
  renderer's building rule (`counted_with`). Next: nothing planned.
- **Scanned leaves and bark** on the generated plants (done for the species that have a set;
  see `vegetation-sources.md` for coverage).
- **Photo mode:** indirect light from sunlit surfaces, crowns keeping their density, a path for
  the browser.
- **Ground detail:** fallen leaves by species and season, stones and twigs on soil and gravel,
  a soft edge where lawn meets border or paving.
- **Four seasons, with snow:** Spring, Summer, Autumn, Winter and Snowy in Sun & sky
  (`view.set {season, snow}`), one plant state per season, the exact date folded away for sun
  studies. Next: snow on ledges narrower than the cover map's texels, falling snow.
- **Sea and large water:** a sea level per project (`water.set_sea`: sea, lake or river, WMO
  sea state, wave direction, colour, clarity, foam, flow, an optional outline) out to the
  horizon, Gerstner waves filtered with distance, foam on crests and along the shore, the wet
  band above the waterline, a beach or bluff along a line (`terrain.shore`), plants kept off
  the bed, wading to 0.6 m, a coastline the endless ground follows (`coast`), surf that
  shoals and breaks over shallow beds with a swash running up the sand, and the `coast`
  example. Next: refraction of the bed, the surf's bed slope measured instead of assumed,
  breakers that lift the surface (they only tilt it now), a jetty or boat that floats (none in
  the library yet).
- **Coast showcase:** the `coast` example, a beachfront house low on a mainland beach with a
  coastal garden round it, on shaped terrain. Next: the same on real coastal topography once ground from
  coordinates exists.
- **Sun hours:** `query.sun_hours` gives the hours of direct sun for a spot (with the intervals)
  or as a grid over an area, on any date or season, with shadow from models, shapes, terrain
  and plant crowns grown `years` (trees, palms, conifers, shrubs and hedges as ellipsoids; bare
  deciduous crowns let 60 % through). Area results come with the share in full sun (6+ h), part
  shade (3-6 h) and shade. The map is shown: `view.set_sun_map` (an area's id or an outline, a season or date,
  `years`, `cell_m`, `opacity`; `view.clear_sun_map`) draws the hours on the ground as a see-through
  colour map (violet to amber, colour-blind safe) in the window and in `view.render`; the window has
  it in Sun & sky (an area or the whole plot, season buttons, years, opacity) with a legend of the
  ramp, the day and the shares. It is a flat sheet at the area's height, also on terrain, and is
  worked out again when the project changes (off the UI thread; in slices a frame on the web).
  Next: plants filtered by the light a spot gets, the map draped on terrain, and what is left out: clouds, crowns as ellipsoids only (no leaf gaps, no climbers,
  perennials or lawn), custom species, the neighbours' volumes (see below).

- **Fences and screens:** `fence.add` draws a line as posts (at every corner, both ends and
  evenly between) with slats, horizontal boards, closed panels, pickets, a wire mesh or a solid
  screen between them, and gates (`fence.add_gate`); faces `<id>:posts`, `:infill`, `:rails`,
  `:gate` take materials per part, `query.quantities` counts the parts and the real volume, and
  Draw has a Fence tool. Next: posts and boards following a slope (a fence stands on the lowest
  ground under its line now, and `shape_mesh_on`'s ground function could set each post's foot),
  stepped panels, a gate that swings, climbers on mesh, and a see-through board gap in
  photo-mode shadows.

- **Levels:** `stairs.add` draws a flight of solid steps between a bottom and a top centre (the
  rise from the ground when left out; steps from a riser height, Blondel's rule checked, a
  landing when the footprint is longer than the treads need; faces `<id>:treads`, `:risers`,
  `:sides`), and `bed.add` a raised bed: a hollow ring of walls plus a soil area inside it that
  plants stand on, in four styles (plain, masonry with a coping, stacked timber sleepers with
  joints and overlapping corners, a thin corten plate with a folded edge). Quantities give steps,
  riser and going, soil volume, beam count and length, plate area; Draw has Stairs and Raised bed
  tools. `area.set_edge` gives a raised area a retaining edge, a skirt in the same four styles from
  its top down to the ground (faces `<id>:edge`, `:edge_rim`; quantities give length, face area,
  beams or plate area), so terraces at different heights no longer float; Draw has an Edge
  selector in the area panel. Stairs turn: `stairs.add` with `via` (one or two corner points)
  makes an L, U or S flight whose corners are square landings of the flight's width, with the
  steps shared over the flights by clear run and each landing at the height of the step reaching
  it; two corners a width apart make a half-landing with the flights side by side, further apart a
  wider landing round a well. The outline is derived from the path, so the vertex commands refuse
  it and `stairs.set` takes `path` or `via` (`null` straightens it); the answer lists `flights`
  and `landings`, quantities count landings and their area and the total going, and the Stairs
  tool has a Turn option that holds each segment at a right angle. Terraces cut into the terrain:
  `terrace.add` makes a flat area whose floor (a height, or the lowest, highest or mean ground
  under it) replaces the ground inside its outline, held by a retaining wall that follows the
  slope along the edge (the floor plus a parapet where it is built up, the ground plus a coping
  where it is cut in; plain, masonry, sleepers, corten or gabion; faces `<id>:wall`,
  `<id>:wall_rim`). The terrain grid is not edited: its chunk meshes get the footprint clipped out
  exactly, and everything that asks for the ground (plants, stairs, beds, fences, raycasts, sun
  hours, the walking camera) gets the floor inside it. The answer and `query.quantities` give cut
  and fill volumes, the wall's length and face areas and its beams or plate, and warn about a wall
  over 1.2 m, no terrain, and a floor that cuts nothing; `terrace.set` and `terrace.clear` change
  or undo it, and Draw has a Terrace tool and the options in the area panel. Next: an edge that
  steps with a draped area, stairs that follow the terrain, winders and curved stairs, a soil
  that follows terrain edited after the bed, walls that step in plan (a terrace with several
  levels sharing one wall), drainage behind a wall, and a wall whose top follows the slope in a
  smooth line for the styles that can.

- **Curves:** a shape keeps its control points and draws a curved outline from them: an arc on
  single edges typed as a radius and a side (`shape.set_curve`, `shape.clear_curve`), or a smooth
  curve through all the points (`shape.set_smooth`), with the same typed-size precision as
  straight edges (a circle of radius 5 measures within 0.1% of a circle). `shape.add_circle`
  makes exact round lawns, ponds and beds; `shape.add_area`, `shape.add_extrusion`, `bed.add` and
  `fence.add` take `curves` and `smooth`; a fence spreads its posts by arc length along a curve,
  and a round plain, masonry or corten bed follows it with soil inside. Draw has a Circle tool, an
  edge panel (pick an edge with Shift-click or a number, type a radius, left or right) and a Smooth
  checkbox. Next: curved houses (walls on many short edges with openings placed by arc length),
  curved sleeper beds and steps, an arc through three picked points, the radius shown and dragged
  on the plan, splitting an arc in two when a corner is added, and an elliptical circle tool.

## Next

- **More leaf sets:** the remaining CC0 atlases, then coverage reported again.
- **Growth data checked against open data,** with the source of each value recorded. The table
  is estimated today.
- **Tree form:** a better branch solver in the plant generator, compared against reference scans
  of real plants.
- **North and site:** turning the whole drawing against north is done (`site.set north_deg`,
  applied to the sun, wind, waves, river flow and lawn stripes). Next: set the site by address
  (`site.set` already takes coordinates), so the sun is right without knowing compass directions.
- **Shade from the surroundings:** done for buildings: a layer marked as surroundings
  (`layer.set_context`) holds neighbours as plain extrusions or models that cast shadow, draw as
  grey massing and stay out of the quantities. Next: terrain beyond the plot as a caster, and
  neighbours from open data once "Ground from coordinates" exists.
- **Ground from coordinates:** give a latitude and longitude (or an address) and get the real
  ground under the model: terrain from an open elevation model, an aerial or satellite image as
  ground texture, and outlines (parcel, buildings, roads, water) where open data has them, with
  coordinates converted to the drawing's metres and north, the site set for the sun, and each
  source's licence and attribution shown. The model is then placed on that ground. Import from
  common geodata files and services as well.
- **Furniture and interiors library:** the parametric pieces are done: furniture generated from
  parameters the way stairs and raised beds are, so a table or a sofa can be given a length
  (`furniture.add`, `furniture.set`, `furniture.clear`). The kinds are table (rectangular, or round
  with a pedestal), chair, bench (with or without a back), sofa and armchair (cushions split by
  length), sun lounger (adjustable back angle), parasol (open or folded), side table, cabinet (doors
  and drawer fronts) and bed (frame, mattress, pillows, headboard), each with stable faces for
  materials (`frame`, `top`, `cushion`, `canopy`) and palette materials of its own.
  `furniture.add_set` places a dining set for 2 to 12 seats, a lounge set or a sunbathing
  set as separate pieces, and `query.quantities` lists the pieces per kind with their dimensions.
  The window has a Furniture tool in Draw. Next: far more furniture than today, indoors and
  outdoors: storage, kitchens and appliances, bathroom fixtures, rugs, curtains, lamps, plants in
  pots, barbecues, with variants in colour and material; more kinds as parametric pieces; pieces
  from open sources (CC0 or CC-BY) as downloadable library assets; placing with snapping to floors
  and walls; grouping as room sets.

## Later

- User species made of several models as growth stages.
- Live reload of a linked 3dcraft model in the browser (desktop has it).
- Interiors and closed buildings.
- Rain and wet surfaces.
- Importers for other CAD sources.
- Hosting the asset library publicly, so the web version works outside a local setup.

## Known gaps

- Nothing has been verified on Windows or macOS.
- Loading still hitches on weak integrated GPUs.
- Tree crowns still read paler from above than from the side: leaves at the top of a crown face
  the noon sun squarely.
- Every mesh level of a broadleaf tree now draws its leaves at 0.7 to 1.4 times their real size
  (`crates/visioncraft-flora/examples/crown_stats.rs` prints the numbers); crowns still show gaps
  between branch systems from the side and read as clumps, because leaves sit only along the
  generated branches (no space colonisation filling the crown yet).
- Bare winter crowns lack a fine fourth branch order: their twig cards read as dark tufts from
  15 m and further, so a winter tree does not yet look like its species.
