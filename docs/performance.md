# Performance: the load timeline

VisionCraft keeps an always-on, cheap record of where loading time goes
(`apps/visioncraft/src/load_stats.rs`). It works the same natively and in the browser
(`performance.now()` there), and is reachable three ways:

- **Agents**: `ui.load` (MCP, `visioncraft-cli mcp --bridge`, the page's `vcRpc`). JSON.
- **Smoke scripts**: the `stats` step of `visioncraft --smoke script.json` prints the table and
  one `lod:` line per placed catalogue asset.
- **Browser console**: every milestone is a line starting `[start]` or `[load]`.

The browser smoke check (`apps/visioncraft/browser-smoke/smoke.mjs`) copies the milestones and
the costliest stages of each example into its `report.json` (`timeline`).

## What `ui.load` returns

| Field | Meaning |
| --- | --- |
| `startup` | The program's start, ms since the program started: `wasm module running`, `window and GPU device ready`, `renderer and pipelines created`, `first frame submitted`, `first frame on screen`, `hardware calibrated`. Never reset. |
| `load.what` | What was opened last (`example coast`, `open garden.visioncraft`, `agent: ...`). Opening a project, an example or `file.new` starts a new load. |
| `load.milestones` | ms since that open: `first frame`, `first frame on screen`, `first plant generated`, `models imported`, `plants done`, `library idle`, `everything loaded`. |
| `load.stages` | Per stage: count, total ms and the longest single step. `breakdown: true` rows detail others and are not part of the total. |
| `load.frames` | Frames drawn since the open, how many took over 25 ms (`over_25ms`, since the last hitch reset) and over 50 ms, and the longest. |
| `load.hitches` | The last 64 frames that took over 25 ms (1.5 frames at 60 Hz): `at_s` (seconds since the program started), `ms`, and `stages`, the main-thread stages timed that frame (`load_stats::time`) that took 0.5 ms or more, as name to ms. A hitch names its cause. Not cleared by opening a project. |
| `load.timeline` | Up to 400 steps of 2 ms or more (the longest win): name, start and duration in ms since the open. |
| `pending` | What is still on its way (plants, models, library, sky volume, sky visibility). |
| `textures` | The texture tier, the store's cap, the content texture memory it is compared with, whether it is over the cap, and what the library holds. |
| `lods` | Per placed catalogue asset, nearest first: the nearest instance's distance, the level of detail that distance needs, the levels resident, the level shown, and in words why it is not the one needed. |

## Measuring hitches

`ui.load` with `{"reset": true}` returns the reply and then clears `hitches`, `over_25ms` and the
frame counters, so the next read covers only what you did. In the browser, `window.__vcFrame`
also carries the worst frame of each 60 for `prepare_max_ms`, `render_max_ms` and `ui_max_ms`,
next to the averages.

Recipe, a time-of-day drag:

1. `await vcRpc("ui.load", {reset: true})` to start clean.
2. Drag the time-of-day slider for 5 s.
3. `(await vcRpc("ui.load")).load.hitches`: each slow frame with the stages that caused it.

## How to read it

- **`first frame on screen` minus `first frame`** is the GPU's part of the first frame, which
  includes compiling every pipeline it uses. In the browser the page's thread does not see this
  cost: the GPU process compiles. On an integrated GPU with an empty shader cache it can be tens
  of seconds; the browser keeps compiled shaders, so a second visit is much faster.
- **Stages named `(main thread)`** stop the frame. Their `longest_ms` is a hitch the person sees.
- **Stages named `(... worker, wall)`** are the time a job took from sending it to the worker to
  its answer, waiting included; several run at once, so they add up to more than the wall time.
- **`browser cache hit` / `miss`** (web, per kind: `mesh` and `tex` the plants, `map` the library
  maps, `picture` the models' pictures, `skyvis` the baked sky visibility) and **`disk cache hit` / `miss`** (native): generated
  plants and compressed maps from an earlier visit. A second visit should be nearly all hits;
  the misses left are what the first visit never asked for. A reload reopens the project at the
  camera it was left at, so after walking among the furniture the second visit asks for its
  finer levels' pictures (on the coast about 60, lod0 and lod1 of some 20 models), which the
  first visit, opened at the overview, never loaded.
- **`quality change: every pipeline made`**: the shading changed (fast shading or not) to one
  the renderer had not made before; changing back is free (`Renderer::set_quality` keeps both).
- **`lods[].why`** other than `the level needed`: still loading, failed, nothing resident yet
  (the grey placeholder), or over the texture memory cap (then only the coarsest and the nearest
  levels are asked for).

## What the loading path does

- **Workers, never the frame.** Natively a work pool sized to the cores; in the browser
  `navigator.hardwareConcurrency - 2` texture workers (3 to 8) make plant meshes and textures
  and block-compress maps; the import worker meshes models and makes the sky volume, and as many
  import workers as texture workers (2 to 8) bake the sky visibility. The page's thread only
  uploads, within a 4 ms budget a frame.
- **Caches.** Natively `plant_cache` on disk; in the browser the Cache API (`web_cache.rs`) with
  the same keys. Keys carry a generator fingerprint (`GENERATOR_FINGERPRINT`: `build.rs` hashes the
  source files listed in `apps/visioncraft/fingerprint-inputs.txt`, the flora crate, `bc.rs` and
  the app's encode, job and compression code), not the build: a changed generator never reads an
  old result, a rebuild or deploy that leaves them alone (a UI change) keeps the cache warm. A
  new input file goes in that list.
- **Pipelines** are made once for the quality the window starts with (the stored hardware
  choice), and per shading after that.
- **Ground clutter** is built once all plants are there, not on the first frame and again after.
- **The sky visibility bake** first looks its result up in the cache (kind `skyvis`, keyed by
  `SkyScene::key`, with a format byte, length and checksum; a damaged entry is a miss), which
  needs only the models (they change the scene) and no cores. On a miss it waits until no plant
  is on its way (it would share the cores with them), then runs on the work pool's share of
  the cores natively and, in the browser, over several import workers that each take the next
  chunk of probes when they answer (`sky_vis.rs`, `SkyBake::probe_floats`): the grid is put
  together in probe order, bit for bit the single bake's. It skips the ground samples of blocks
  a ray passes above, and tests per ray only the crowns whose cones reach its elevation.
- **Plant meshes** grow about twice as fast as before with the same bytes: the nearest crown
  ellipsoid comes from a grid of the few that can win (`flora/src/near.rs`), space colonisation
  skips dead attraction points and finished nodes, and the wood plan and tubes stop redoing
  work. A grown tree variant takes 50 to 255 ms natively (it was 100 to 600).
- **Draws** skip re-binding what is bound (meshes, instances, materials, slots), and the
  impostors' draw slots are written when they change, not every frame.

## What a frame does

- **One instance pool, batched draws** (`batch.rs`). Every item's instances live in one buffer.
  A frame plans all its passes before recording any (`plan_items`): per pass each cluster's
  level is found once and an item's runs once per level, then the visible runs of every item that
  shares a pipeline, a material and a mesh (the same species in several plantings, the same rock
  or chair placed many times, all impostors of one atlas) become one draw. Their pool indices go
  into one list that a compute dispatch copies into a frame buffer before the first pass, nearest
  first. Dissolving clusters and terrain chunks draw straight from the pool. Nothing in the
  shaders changed: the gather copies the instance bits.
- **Shadows**: the near cascade every frame, the middle ones on alternate frames, the last one
  cached while the camera stays within its margin, each culled by its own cascade. The snow
  cover map (on a coast also the sea's bed) follows the eye in steps of eight texels and is
  drawn again only when it moves or the scene changes.
  **While the sun moves** (a dragged time of day) `set_sun` no longer drops the caches: every
  cascade keeps its map and matrix and is marked stale, so it shows the shadow of a sun a few
  frames old, consistently. The near cascade still draws every frame, the cached far one every
  fourth frame (at once when the camera leaves its margin), the middle ones on their alternate
  frames; the snow cover map does not depend on the sun and is not redrawn. The frame after the
  sun stops, every stale cascade draws, so the resting picture is the same as before.
  `finish_environment` (a picture of the live view) forces that refresh too; the offscreen
  stills start without any cache. The rule is `cascade_refresh` in `visioncraft-render`.
- **The water reflection** is drawn only when the view frames water: the sea counts when a grid
  of rays through the view lands on its side of the coast (`sea::sea_in_view`), a pond when its
  clusters are in view. It draws the coarser levels, and leaves out what would be under 1.5
  pixels of radius in its image (`MIN_REFLECTED_PX`): far herbs, stones and chairs.

## Measured (November 2026)

Orca-server (Intel Iris Xe, 8 threads), the four examples, `wait assets timed 1280 720` natively
and the browser smoke check (headless Chromium, WebGPU over Vulkan, 1600 x 900, auto quality).
The machine was shared with other jobs, so single numbers move by 20 to 50 %.

Browser, until everything arrived (s), first visit / second visit:

| Example | Before | After |
| --- | --- | --- |
| garden | 25.5 / (no cache) | 20.5 / 2.5 |
| estate | 34.8, then 8 fps while the sky bake ran on the page | 17.6, 60 fps |
| beach-house | 24.5 | 11.7 |
| coast | 49.0, then 5 fps | 15.0 / 9.9, 27 to 56 fps |

The first frame reached the screen 0.1 to 1.1 s after opening an example in the browser.

### Coast (the main benchmark)

Browser, two interleaved runs each of the build before and after (the machine was shared, so
the spread is wide); load until nothing is pending but the sky visibility, then until the sky
visibility is in, then frames while the camera circles at four viewpoints (MEDIUM, auto scale):

| | Before | After |
| --- | --- | --- |
| loaded (s) | 64.8, 119.9 | 59.4, 40.1 |
| sky visibility in (s) | 126, 150 (baked on the page's thread) | 96, 83 (in the worker) |
| frames while it bakes | 191 ms CPU per frame (5 fps) | no cost on the page |
| overview frame, median / p90 (ms) | 38.5 / 68.9, 34.0 / 56.3 | 22.2 / 42.9, 16.6 / 20.3 |
| CPU recording a frame (ms) | 17 to 27 | 6 to 21 |

The GPU part (timestamp queries, Iris Xe, MEDIUM, 1600 x 900 at 55 to 75 % scale): forward
16 to 35 ms, depth 1 to 5, reflection 1 to 3, shadows 0.5 to 2.2, SSAO 0.5, the post chain
about 1.5. Natively (`bench` in a smoke script, quiet machine): recording 6 to 7 ms and submit
6 ms for 1,450 to 1,700 draws. The coast is GPU-bound on an integrated GPU; on a desktop GPU the
CPU recording of its 1,500 to 2,300 draws (each a few calls into the browser's WebGPU) is what
remains.

Native, first frame / everything loaded (s), empty disk cache, before this round:

| Example | Before | After |
| --- | --- | --- |
| garden | 0.76 / 8.1 | 0.38 / 7.1 |
| estate | 13.9 / 43.7 | 2.4 / 14.8 |
| beach-house | 0.76 / 4.3 | 0.82 / 4.4 |
| coast | 1.0 / 16.2 | 2.8 / 18.5 |

### Second round: draws, the reflection, the bake over workers (November 2026)

The same machine, before (the commit before this round, `4f235b7`) and after, both built with the
release profile of thin LTO and one codegen unit and run against the same (re-ingested) library,
interleaved. Other agents shared the machine (load 2 to 20), so the browser's times move a lot;
the draw counts do not.

Coast, native (`bench 120 1600 900`, the overview `53.83 34.61 33.42 -> 10.1 1.87 -30.5` and a
walking spot by the house `10.43 1.71 -29.57`), two runs each:

| | before | after |
| --- | --- | --- |
| MEDIUM overview: draws (shadow / depth / reflection / forward) | 2398 (638 / 626 / 487 / 636) | 469 (110 / 118 / 102 / 128) |
| MEDIUM overview: frame median ms | 36.6, 37.6 | 34.2, 34.8 |
| MEDIUM overview: GPU reflection ms | 2.5, 2.6 | 1.2, 1.2 |
| MEDIUM walking: draws | 1352 | 177 |
| MEDIUM walking: frame median ms | 55.8, 55.8 | 30.0, 30.6 |
| MEDIUM walking: CPU record + submit ms | 8.4 + 13.9, 8.2 + 13.2 | 2.2 + 2.8, 2.1 + 2.8 |
| HIGH overview: draws | 1985, 2467 | 518 |
| HIGH overview: frame median ms | 103, 110 | 97, 96 |
| HIGH walking: draws | 2035, 2125 | 269 |
| HIGH walking: frame median ms | 91, 123 | 70, 69 |

Walking by the house the view does not frame the sea, so the reflection pass is gone; the
overview's view reaches the sea's side of the coast, so it keeps the pass, with fewer things in
it.

Coast, browser (headless Chromium, WebGPU on the Iris Xe, MEDIUM, 1600 x 900; frames are held
to 60 Hz, so the medians sit at 16.7 ms on both): the CPU a frame spends in `render_part`, from
a 5 s profile of the page while the camera circles (`Profiler` over the DevTools protocol, a
build without `wasm-opt` for the names):

| | before | after |
| --- | --- | --- |
| overview: draws | 2,500 to 3,900 | 410 to 650 |
| overview: `render_part` ms a frame | 7.0 | 4.7 |
| walking: draws | 1,340 to 1,560 | 177 to 290 |
| walking: `render_part` ms a frame | 4.4 | 3.3 |

What is left of a frame's CPU is mostly `plan_items` itself (the clusters' visibility and
levels: half to two thirds of `render_part`); the draws are a sixth of what they were.

Coast in the browser, load (s), three interleaved pairs, cold first visit / second visit (a
reload):

| | before | after |
| --- | --- | --- |
| plants done, first visit | 29.4, 44.9, 27.8 | 26.0, 38.7, 37.2 |
| sky visibility in, first visit | 79.8, 117.4, 64.8 | 54.9, 78.0, 72.4 |
| sky visibility in, second visit | 47.7, 97.4, 49.9 | 27.2, 49.8, 37.1 |

The quietest pair (load 2 to 6) is the first number of each. The bake's own wall time: one
import worker took 46 to 106 s (71 s in the quiet pair), six now take 21 to 39 s (28 s), and the
bake starts once, after the models (before, the models' arrival made its scene a second time).

A texture worker queue on the page (one job a worker, most urgent first) was tried and left out:
on the coast it delayed `plants done` by 3 to 4 s in three interleaved pairs, probably because
it put the model pictures before every plant.
