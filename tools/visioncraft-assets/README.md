# visioncraft-assets

Offline tool that builds the openly licensed asset library (`assets/` at the repo root): it
fetches objects, materials and skies from their sources, normalises them and writes the
catalogue. Native only; it is a workspace member but never built for wasm.

## Commands

All commands take `--user-agent` (default `asset-ingest/0.1`, sent to the asset APIs; keep personal
data out of it) and `--delay-ms` (pause between requests, default 500).

- `fetch --list <candidates.json>... [--only id...] --cache assets/.cache` downloads the sources,
  verifying md5 or size and skipping what is already cached.
- `ingest --cache assets/.cache --out assets [-j 3]` writes `models/`, `materials/`, `skies/`.
- `fetch --tier 2k|4k` and `ingest --tier 2k|4k` build the higher tiers next to the base set: materials
  2k and 4k (`materials/<id>/2k/`, `4k/`), models `lod0_2k.glb`, skies `sky_4k.hdr`. `catalog` lists them
  in `tiers.json`. The base set is never replaced.
- `fetch --flora candidates/flora.json [--tier 2k]` and `ingest --flora candidates/flora.json [--tier 2k]`
  build the scanned leaf atlases and bark sets of the plant generator (`assets/flora/`); `catalog`
  writes `flora.json` (sets, leaf cut-outs, species mapping) and the flora part of `tiers.json`.
- `catalog --out assets` writes `catalog.json`, `materials.json`, `skies.json`, `ATTRIBUTION.md`.
- `check --out assets` re-validates everything on disk and exits non-zero on any problem:
  `cargo run -p visioncraft-assets -- check --out assets`. Run it in CI after `catalog`.
- `search --source polyhaven|ambientcg --query text` and
  `add --source ... --id ... --category ... --list candidates/<file>.json` extend a candidates file.
- `lightsheet`, `lodcmp`, `sheet` render inspection images (emitter markers, LODs, thumbnails).

## Sources

- Poly Haven (<https://polyhaven.com>): everything CC0. The API needs a distinctive User-Agent and
  no key; be gentle, the tool is sequential and caches everything.
- ambientCG (<https://ambientcg.com>): everything CC0. One person runs the site: do not hammer it.
- Not allowed: anything that is not CC0 or CC-BY 4.0 (`check` rejects it), e.g. Share Textures.

## Adding an asset

1. `search` for it, then `add` it to a file in `candidates/` (set `category`; materials also
   need `kind`; models may carry `tags`, plant `species`, and `exclude` for bad variants).
2. `fetch --list candidates/*.json`, `ingest`, `catalog`, `check`.
3. Look at the thumbnail (`sheet`); drop it with a line in `candidates/removed_after_review.tsv`
   if it is broken.

## Output notes

Paths in the JSON files are relative to the library root. A model has `lod0..2.glb`
(textures at 1024, 512, 256 px; lod0 at most 10,000 triangles, plants 20,000), `thumb.png` and a
catalogue entry. Light fixtures carry `light.offset_m` (emitter position relative to the pivot);
the emitting materials are named `<name>_emitter` and are emissive.
