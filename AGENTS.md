# VisionCraft

Real-time visualisation of designs: load or draw a house or garden, add materials, plants and
lights from an open-source library, and walk through it. Browser first (WebGPU), desktop second.
Reads 3dcraft (`.3dcraft`) designs; other CAD sources may follow.

## Who does what

- **Fable orchestrates.** The main session plans, writes shared scaffolding, briefs agents,
  verifies their results and reports. It does not do bulk work itself.
- **Sonnet and Haiku agents do bulk work:** implementation, builds, test runs, file surveys,
  asset ingestion.
- **Opus agents do thinking work:** design questions, API discovery in unfamiliar code,
  rendering maths, reviews.
- Agents working in parallel each own their directories. Only the orchestrator edits shared
  files (the root `Cargo.toml`, this file).

## Build server

- The default build server is `dokploy-slave` (100.64.0.7, user `edugo`, SSH alias
  `dokploy-slave`): 32 cores, 377 GB RAM. Run heavy builds and test runs there
  (`cargo build`, `cargo clippy`, `cargo test`, the wasm32 build) rather than on `orca-server`, the local machine the agents run on (8 cores, Intel Iris Xe).
- Reach it with `ssh dokploy-slave`; the `HostKeyAlias` in the SSH config means the bare IP fails
  host key verification.
- Sync the working tree with `rsync -az --delete --exclude target --exclude .claude ./
  dokploy-slave:visioncraft/`, then run cargo there after `. ~/.cargo/env`. Start long runs
  detached (`nohup ... > ~/check.log 2>&1 &`) so a dropped connection does not kill them.
- The server has no GPU: wgpu falls back to a software adapter there, so the pixel references in
  `crates/visioncraft-render/tests/reference.rs`, the GPU recipe renders
  (`cargo test -p visioncraft-cli --test cli -- --ignored`) and visual check renders run on
  `orca-server`. On the build server, run
  `cargo test --workspace --exclude visioncraft-render && cargo test -p visioncraft-render --lib`.

## Rules

1. Own wgpu renderer. No Bevy, no game engine, no ECS crate.
2. Three layers, dependencies only downwards:
   `apps/visioncraft` (window, HUD) -> `crates/visioncraft-render` (wgpu, no UI) ->
   `crates/visioncraft-core` (headless, no GPU).
3. `unsafe_code = "forbid"` in every crate.
4. Never break the `wasm32-unknown-unknown` build. A milestone is done when it runs in
   Chromium on WebGPU.
5. Same stack and versions as 3dcraft where we overlap: wgpu, eframe/egui, WGSL.
6. Clean room: work only from public specifications, papers and openly licensed code. Nothing
   from closed visualisation packages: no assets, shaders, UI designs or file formats.
7. Assets are CC0 or CC-BY only, each with licence, author and source recorded.
8. Everything in this repository is in English.
9. Everything is a command. Any change a person can make in the app runs through a `Session`
   command with JSON parameters, and every command is reachable from `visioncraft-cli` and over
   MCP, in the same manner as 3dcraft (`crates/visioncraft-automation/docs/`). A feature that can
   only be used with the mouse is not finished.
