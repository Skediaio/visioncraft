# Roadmap

What is planned, in rough order. Updated 2026-11-14. Items move up when they are picked up;
nothing here is a promise of a date.

## In progress

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

## Next

- **More leaf sets:** the remaining CC0 atlases, then coverage reported again.
- **Growth data checked against open data,** with the source of each value recorded. The table
  is estimated today.
- **Weathering:** dirt, moss and wear on walls and paving, so materials stop looking new.
- **Tree form:** a better branch solver in the plant generator, compared against reference scans
  of real plants.
- **North and site:** turning the whole drawing against north is done (`site.set north_deg`,
  applied to the sun, wind, waves, river flow and lawn stripes). Next: set the site by address
  (`site.set` already takes coordinates), so the sun is right without knowing compass directions.
- **Sun hours:** hours of sun per day for any spot or area, as a number and as a map over the
  garden, by season and with grown plants; plants filtered by the light a spot gets.
- **Shade from the surroundings:** neighbouring buildings and terrain as simple volumes that cast
  shadow but are not part of the design.
- **A plan as underlay:** a scaled image or PDF page of a floor plan or site plan under the
  drawing, to trace over.
- **Photos on walls:** a facade photo as the texture of a wall, straightened and scaled.
- **Ground from coordinates:** give a latitude and longitude (or an address) and get the real
  ground under the model: terrain from an open elevation model, an aerial or satellite image as
  ground texture, and outlines (parcel, buildings, roads, water) where open data has them, with
  coordinates converted to the drawing's metres and north, the site set for the sun, and each
  source's licence and attribution shown. The model is then placed on that ground. Import from
  common geodata files and services as well.
- **Films from camera moves:** a film described as commands, so a person or an agent can compose
  it: a camera path through points with a speed, height and look-at target per stretch (walk,
  glide, rise, orbit round a spot, a drone-like fly-over), eased starts and stops, the sun's
  course, a season or the years of growth changing along the way, rendered to numbered frames or
  a video file. An agent gets the garden's layout from the same queries it designs with and
  proposes the shots.
- **Quantities:** areas, lengths, volumes and counts per object and per material as a query, so
  tools built on top can list what a design contains.

- **Curves:** outlines with arcs and smooth curves for paths, borders and lawns, with the same
  typed-size precision as straight edges.
- **Levels:** steps and stairs, retaining walls, raised beds and terraces at different heights
  as first-class shapes.
- **Fences and screens:** generated along a line (slats, panels, posts, gates), like hedges.
- **Denser borders:** layered perennial plantings that close up, with flowers that read from
  eye level.

- **Furniture and interiors library:** far more furniture than today, indoors and outdoors:
  sofas, chairs, tables, beds, storage, kitchens and appliances, bathroom fixtures, rugs,
  curtains, lamps, plants in pots, garden sets, loungers, parasols, barbecues, with variants in
  size, colour and material. From open sources (CC0 or CC-BY) and as parametric pieces generated
  like plants are, so a sofa or a table can be given a length. Placed with snapping to floors and
  walls, grouped as room sets, and listed with dimensions.

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
