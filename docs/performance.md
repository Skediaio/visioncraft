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
| `load.frames` | Frames drawn since the open, how many took over 50 ms, and the longest. |
| `load.timeline` | Up to 400 steps of 2 ms or more (the longest win): name, start and duration in ms since the open. |
| `pending` | What is still on its way (plants, models, library, sky volume, sky visibility). |
| `textures` | The texture tier, the store's cap, the content texture memory it is compared with, whether it is over the cap, and what the library holds. |
| `lods` | Per placed catalogue asset, nearest first: the nearest instance's distance, the level of detail that distance needs, the levels resident, the level shown, and in words why it is not the one needed. |

## How to read it

- **`first frame on screen` minus `first frame`** is the GPU's part of the first frame, which
  includes compiling every pipeline it uses. In the browser the page's thread does not see this
  cost: the GPU process compiles. On an integrated GPU with an empty shader cache it can be tens
  of seconds; the browser keeps compiled shaders, so a second visit is much faster.
- **Stages named `(main thread)`** stop the frame. Their `longest_ms` is a hitch the person sees.
- **Stages named `(... worker, wall)`** are the time a job took from sending it to the worker to
  its answer, waiting included; several run at once, so they add up to more than the wall time.
- **`browser cache hit` / `miss`** (web) and **`disk cache hit` / `miss`** (native): generated
  plants and compressed maps from an earlier visit. A second visit should be nearly all hits.
- **`quality change: every pipeline made`**: the shading changed (fast shading or not) to one
  the renderer had not made before; changing back is free (`Renderer::set_quality` keeps both).
- **`lods[].why`** other than `the level needed`: still loading, failed, nothing resident yet
  (the grey placeholder), or over the texture memory cap (then only the coarsest and the nearest
  levels are asked for).

## What the loading path does

- **Workers, never the frame.** Natively a work pool sized to the cores; in the browser
  `navigator.hardwareConcurrency - 2` texture workers (3 to 8) make plant meshes and textures
  and block-compress maps; the import worker meshes models, makes the sky volume and bakes the
  sky visibility. The page's thread only uploads, within a 4 ms budget a frame.
- **Caches.** Natively `plant_cache` on disk; in the browser the Cache API (`web_cache.rs`) with
  the same keys plus the build's wasm hash, so a rebuilt generator never reads an old result.
- **Pipelines** are made once for the quality the window starts with (the stored hardware
  choice), and per shading after that.
- **Ground clutter** is built once all plants are there, not on the first frame and again after.

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

Native, first frame / everything loaded (s), empty disk cache:

| Example | Before | After |
| --- | --- | --- |
| garden | 0.76 / 8.1 | 0.38 / 7.1 |
| estate | 13.9 / 43.7 | 2.4 / 14.8 |
| beach-house | 0.76 / 4.3 | 0.82 / 4.4 |
| coast | 1.0 / 16.2 | 2.8 / 18.5 |
