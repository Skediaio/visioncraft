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
  Plants are filtered by the light a spot gets: every built-in species has an estimated light
  range (the hours of direct sun on a summer day it thrives in: full sun 6+ h, part shade 3-6 h,
  full shade under 3 h; the lawn wants 4+ h; a species outside the table gets a guess from its
  grower type), `query.plants_for_light` ranks the species for a point, an outline (by its mean
  hours) or given hours (fit `full` inside the range, `partial` within one class), `query.light_check`
  measures the sun at every placed plant (its own planting's crowns excluded) and lists the ones in
  too much or too little light with 2-3 species of the same grower type to swap in, and
  `planting.add_border` takes `style: auto` (shade under 3 h, cottage up to 6 h, prairie or dry above) and
  warns with `light_fit` when an explicit style clearly mismatches the measured light. The window
  has "Plants for this light" and "Check plant light" under the map, with a ring on the plan for each
  misfit. The light values are estimated and not yet checked against open data.
  Next: the map draped on terrain, and what is left out: clouds, crowns as ellipsoids only (no leaf gaps, no climbers,
  perennials or lawn), custom species, the neighbours' volumes (see below).

- **Fences and screens:** `fence.add` draws a line as posts (at every corner, both ends and
  evenly between) with slats, horizontal boards, closed panels, pickets, a wire mesh or a solid
  screen between them, and gates (`fence.add_gate`); faces `<id>:posts`, `:infill`, `:rails`,
  `:gate` take materials per part, `query.quantities` counts the parts and the real volume, and
  Draw has a Fence tool. On a slope `slope` makes a fence follow the ground (the terrace-aware
  ground function sets each post's foot): `stepped` stands every post on its own ground with a
  level panel per span at the higher post's ground (and an optional gravel board under it),
  `raked` runs the rails and the top and bottom lines between the feet with plumb slats and
  pickets cut to them, boards parallel to the rake and sheared panels and mesh; gate leaves stay
  level and warn over rising ground, and Draw has a slope list and a gravel-board checkbox.
  Next: a gate that swings, climbers on mesh, and a see-through board gap in photo-mode shadows.

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
  or undo it, and Draw has a Terrace tool and the options in the area panel. Stairs follow the
  terrain: `set_in` on `stairs.add` and `stairs.set` sets a flight into the slope it climbs. The
  step count is still the rise over the riser, but each riser stands where the original ground
  (terrain and terraces, along the centreline) has climbed to the level of the step before, so a
  gentle stretch is a long tread and a steep one a short one, each held within 0.25 and 2 m and
  warned about when it is held or when a going outside 0.28 to 0.45 m fails Blondel's rule; a
  flight that turns does it per flight and keeps its landings. The footprint, widened at the sides
  by the cheeks, is cut out of the terrain's chunk meshes like a terrace's and the ground inside
  it is the tread's level, so nothing grows on the steps. Cheek walls stand along both sides in
  any wall style (faces `<id>:cheeks`, `:cheeks_rim`), from below the step up to the ground beside
  it plus a coping where they hold earth and just under the tread where the ground is lower, or
  none at all, which warns about the bank. The answer and `query.quantities` give the goings, the
  cheek length and face areas, and cut and fill; Draw has a Set into the slope checkbox and the
  cheek styles in the stairs panel. Next: an edge that
  steps with a draped area, winders and curved stairs, a soil
  that follows terrain edited after the bed, walls that step in plan (a terrace with several
  levels sharing one wall), drainage behind a wall, and a wall whose top follows the slope in a
  smooth line for the styles that can.

- **Curves:** a shape keeps its control points and draws a curved outline from them: an arc on
  single edges typed as a radius and a side, a sagitta or a point it passes through
  (`shape.set_curve`, `shape.clear_curve`), or a smooth curve through all the points
  (`shape.set_smooth`), with the same typed-size precision as straight edges (a circle of radius
  5 measures within 0.1% of a circle). `shape.add_circle` makes exact round lawns, ponds and beds
  and `shape.add_ellipse` elliptical ones (control points on the ellipse with an arc between each
  pair, as few as measure within 0.1% of the true area and perimeter); `shape.add_area`,
  `shape.add_extrusion`, `bed.add` and `fence.add` take `curves` and `smooth`; a fence spreads its
  posts by arc length along a curve, and a round bed follows it with soil inside in any style.
  Sleeper beds, sleeper edges on raised areas and sleeper terrace walls take curves: the beams are
  chords of the curve, the fewest equal ones per course that keep each within 2 cm of the arc and
  under 2.4 m (28 a course on a 3 m radius bed), with mitred ends so no gap shows, the joints
  staggered by half a beam from course to course, and `beams` and `beam_length_m` in the
  quantities. `shape.insert_vertex` on an arc splits it into two arcs on the same circle, the new
  point projected onto it. A flight of steps takes an arc on its bottom edge (`front_bow_m` on
  `stairs.add` and `stairs.set`, or `shape.set_curve`): the risers are concentric arcs, a round
  or hollow front, the goings counted from the apex. Draw has Circle and Ellipse tools, an edge
  panel (pick an edge with Shift-click or a number, type a radius, left or right), a Smooth
  checkbox, the radius of every arc on the plan and a handle in the middle of each arc to drag
  (one `shape.set_curve` on release). A house takes curves: the walls follow
  the tessellated outline as many short pieces of one solid, the flat roof's parapet is the outline
  offset and mitred so it keeps its width, the roof deck and floor follow the curve, and a window or
  door on a curved edge is placed by arc length (offset and width along the arc) as a flat unit on
  the chord of its span with planar reveals, narrowed with a note when the arc is so tight that its
  chord would leave the 0.3 m wall; `query.quantities` gives a house's wall length by true arc
  length and its wall area less the openings; a pitched roof on a curved outline is drawn flat with
  a note. Recipes 23, 35 and 37. Next: pitched roofs on a curved outline, an arc on the top edge of a flight (and so
  fans of treads that converge), curved stairs that turn or are set into the slope, curved
  cheek walls, and a rotation handle for an ellipse.
- **Rain and wet surfaces:** `view.set {wetness: 0..1, rain: 0..1}` (both 0 by default and not
  written then; rain implies wetness, the one drawn is the larger of the two), also per picture in
  `view.render` (`--wetness`, `--rain` in the CLI) and as `[from, to]` keys of a film shot, so a
  shot eases from dry into rain. Wet porous surfaces (soil, wood, stone, concrete, brick, plaster;
  porosity follows from the roughness and metalness a material already has) darken about as the
  square of their albedo and turn glossy, leaves take a sheen, metal and glass hardly change.
  From about 0.5 wet, puddles form on flat, open, upward-facing paving, terraces, paths and soil
  (not lawns, plants, slopes or ground under a roof or crown, by the snow cover map) in
  low-frequency world-space patches that grow with the value, as smooth water films mirroring the
  sky and the buildings. Rain draws depth-tested streaks in five screen-space layers slanted by
  the wind, fewer under cover, with ripple rings on puddles, ponds and lakes; the sky closes to
  a grey overcast layer and the sun weakens with it. All in the renderer, a few noise lookups
  and nothing when off (the pixel references are unchanged). In the window: Wetness and Rain
  sliders under Sun & sky. Recipe 36. Next: drips and runoff from roofs and edges, splashes where
  drops land, wet-surface darkening that follows the lowest points (puddles in hollows of the
  terrain), steam after rain, a per-material porosity, and rain on glass.
- **Atmosphere and wind (done):** `view.set {mist: 0..1, gustiness: 0..1}` (0 by default and not
  written then), also per picture in `view.render` (`--haze`, `--mist`, `--gustiness` in the CLI)
  and as `[from, to]` keys of a film shot (with `haze`). Mist is a second exponential height-fog
  layer of droplets under the dry-air haze (scale height 25 m; visibility 1.3 km at 0.3, 330 m at
  0.6, 120 m at 1), and rain adds its own (400 m at 0.8, scale height 600 m): both veil every
  surface, water, the rain streaks and the sky near the horizon in light scattered in from the sun
  (Henyey-Greenstein, g = 0.75, lit by the sun share that reaches the ground, so a grey veil in rain
  and a white haze with a glow towards a clear low sun) and from the sky, and dim the sun on the
  ground by the layer's optical depth. Gusts are one world-space field, two layers of value noise
  carried downwind at the wind speed: lawn grass bends with them (up to about 50 degrees in a
  strong gust's core) and its tips turn paler where a gust passes, so gusts roll over a lawn as
  waves; trunks, branches and leaves take the field at the foot and at each point, with a phase per
  plant; the rain's slant swings with it. A CPU mirror of the sway with gusts is checked against
  the shader. Mist 0 and gusts 0 draw exactly what was drawn before (the pixel references are
  unchanged). In the window: Mist and Gusts sliders under Sun & sky. Recipe 39. Next: volumetric
  clouds, rain particles as a 3D volume with splashes, drops on all wet surfaces and runoff on
  walls, wetness that builds up and dries over time, lightning, and tone mapping and grading.

## Next

- **Photographic quality (on par with commercial visualisers):** a side-by-side with public
  showcase renders of a commercial visualiser showed the gaps, in order of how much they show.
  Sources and costs are in the research report (kept outside the repo). Done first: analytic
  height fog with in-scattering and travelling wind gusts (see the rain item).
  1. Light and mood (AgX, threshold-free bloom, SSAO with contact shadows and EV100 exposure are
     already in): light shafts at a low sun (a half-resolution march against the four cascades),
     an aerial perspective LUT so the haze takes the sun's tint, a wider bloom for the sun glow,
     white balance from the sky and a grading LUT before AgX, and a photo mode that accumulates
     jittered frames (real anti-aliasing, depth of field, motion blur, light vignette and grain).
  2. Materials and ground: download the catalogued CC0 library (ambientCG, Poly Haven) into
     checkouts, hex tiling and height blending on terrain and paving, joints and relief (boards,
     tiles, parallax up close), and leaf litter, pebbles and moss as scatter that follows the season.
  3. Plants: lawn blades generated in compute with clumping and backlit translucency (as in
     Ghost of Tsushima, GDC 2021), tall and ornamental grasses that glow against the sun, flowers
     and fine leaves up close (more leaf sets), and fuller crowns (see Tree form).
  4. Light in the shade: sky visibility baked per design change as SH and multiplied by the live
     sky (Ghost of Tsushima, Advances 2021), GTAO with specular occlusion, soft shadows (PCSS on the
     four cascades, contact shadows, cloud shadows), SSR with a sky fallback for water, windows and
     puddles, and later probe GI (DDGI traced against an SDF or voxels). Lumen is out of reach in
     the browser.
  5. Life: people, a furniture library of real CC0 models next to the parametric pieces, small
     props, falling leaves and birds as particles, and warm lit interiors behind windows at dusk.
  6. Weather, finished: a top-down rain occlusion map, wetness that builds up and dries with
     puddles that grow from their centre, drops on every wet surface, volumetric clouds after Nubis
     (2015/2017) with weather transitions, and lightning.
- **More leaf sets:** the remaining CC0 atlases, then coverage reported again.
- **Growth data checked against open data,** with the source of each value recorded. The table
  is estimated today, and so are the light needs per species (`plant_light.rs`: the hours of direct
  sun each plant thrives in), which are to be checked and sourced the same way.
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
