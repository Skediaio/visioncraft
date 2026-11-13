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
