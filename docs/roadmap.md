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

## Next

- **More leaf sets:** the remaining CC0 atlases, then coverage reported again.
- **Growth data checked against open data,** with the source of each value recorded. The table
  is estimated today.
- **Weathering:** dirt, moss and wear on walls and paving, so materials stop looking new.
- **Tree form:** a better branch solver in the plant generator, compared against reference scans
  of real plants.
- **North and site:** turn the whole drawing against north after the fact, and set the site by
  address or coordinates, so the sun is right without knowing compass directions.
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
- **Sea and large water:** a sea, lake or wide river to the horizon: waves and swell driven by
  the wind setting, a shoreline with foam where water meets land, depth colour, reflections and
  sun glitter, a beach or bank as terrain; boats and jetties from the library.
- **Coast showcase:** an example of a beachfront house looking out over the sea, first on
  shaped terrain, then on real coastal topography once ground from coordinates exists.
- **Quantities:** areas, lengths, volumes and counts per object and per material as a query, so
  tools built on top can list what a design contains.

- **Curves:** outlines with arcs and smooth curves for paths, borders and lawns, with the same
  typed-size precision as straight edges.
- **Levels:** steps and stairs, retaining walls, raised beds and terraces at different heights
  as first-class shapes.
- **Fences and screens:** generated along a line (slats, panels, posts, gates), like hedges.
- **Denser borders:** layered perennial plantings that close up, with flowers that read from
  eye level.

## Later

- User species made of several models as growth stages.
- A film along viewpoints, with growth or the sun's course as animation.
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
