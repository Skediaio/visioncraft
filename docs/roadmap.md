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

## Next

- **Four seasons, with snow.** One choice of Spring, Summer, Autumn or Winter instead of twelve
  monthly plant states; the exact date stays available for sun studies. Winter can have snow:
  on the ground, roofs, hedge tops, branches and sills, not under roofs and dense evergreens.
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
- **Map data as a start:** import of outlines (parcel, buildings), a height raster as terrain and
  an aerial image as ground texture from common geodata formats and services, with coordinates
  converted to the drawing's metres and north.
- **Quantities:** areas, lengths, volumes and counts per object and per material as a query, so
  tools built on top can list what a design contains.

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
- Photo mode is desktop only.
- Loading still hitches on weak integrated GPUs.
- Tree crowns read pale in the plan view.
