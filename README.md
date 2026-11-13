# VisionCraft

Real-time visualisation of houses and gardens. Draw a design or load a `.3dcraft` file, add
materials, plants and lights from an open-source asset library, shape the terrain, walk through
the result and export pictures. It runs in the browser on WebGPU and as a desktop application.

Every action a person can take is a command with JSON parameters, and every command is reachable
from `visioncraft-cli` and over MCP (see [CLI and MCP](#cli-and-mcp)).

Status: version 0.0.0, not published.

## Workspace layout

Dependencies point downwards only.

| Path | What it is |
|---|---|
| `apps/visioncraft` | The application (egui/eframe window, HUD). One crate for native and for the `wasm32-unknown-unknown` build. |
| `apps/visioncraft-cli` | `visioncraft-cli`: headless command line, JSON-lines server and MCP server. |
| `crates/visioncraft-render` | wgpu renderer, WGSL shaders. No UI. |
| `crates/visioncraft-core` | Headless core: project document, commands with undo, model import, sun, ray queries. No GPU, no filesystem. |
| `crates/visioncraft-flora` | Procedural, deterministic plant meshes and textures. |
| `crates/visioncraft-automation` | Command execution, JSON-lines and MCP protocols, control channel to a running window. |
| `tools/visioncraft-assets` | Offline tool that builds the asset library. |
| `assets/` | The asset library (not in git, see below) and its [attribution](assets/ATTRIBUTION.md). |

Rules are in [AGENTS.md](AGENTS.md). Code attribution is in [ATTRIBUTION.md](ATTRIBUTION.md).

## Build and run

Needs a recent stable Rust toolchain. `.3dcraft` support comes from the `craft3d-*` crates, which
are fetched from <https://github.com/johancoppens/3dcraft> by Cargo.

Native:

```
cargo run -p visioncraft --release -- --assets assets
```

`--assets <dir>` points at the asset library; without it the app looks for `./assets`, then for
`assets` beside the executable. Without a library it starts, with objects and library materials
unavailable.

Web, with [Trunk](https://trunkrs.dev) and the `wasm32-unknown-unknown` target
(`rustup target add wasm32-unknown-unknown`):

```
cd apps/visioncraft
trunk serve --release
```

`apps/visioncraft/index.html` builds two binaries of the crate: `visioncraft` and `import_worker`
(a Web Worker that imports models off the page thread). There is no `Trunk.toml`. The page loads
the library with `fetch` from `./assets/` beside the page; `?assets=<url>` or `window.VC_ASSETS`
override that. Trunk does not copy `assets/` by itself, so serve the library next to the page or
pass `?assets=`.

## Browser requirements

WebGPU only; there is no WebGL fallback. Chrome, Edge and Brave on Windows, macOS, ChromeOS and
Android are expected to work when up to date. On Linux with Chromium-based browsers, WebGPU may
need these flags (also available at `chrome://flags`):

```
--enable-unsafe-webgpu --enable-features=Vulkan --ignore-gpu-blocklist
```

Check `chrome://gpu` for "WebGPU: Hardware accelerated". Firefox and Safari depend on their
version. The page shows a message when WebGPU is missing.

## Assets

Assets are not in git. Objects, materials and HDR skies come from Poly Haven and ambientCG (both
CC0); only CC0 and CC-BY 4.0 are accepted. `tools/visioncraft-assets` builds the library (native
only), see its [README](tools/visioncraft-assets/README.md):

```
cargo run -p visioncraft-assets -- fetch --list tools/visioncraft-assets/candidates/*.json --cache assets/.cache
cargo run -p visioncraft-assets -- ingest --cache assets/.cache --out assets
cargo run -p visioncraft-assets -- catalog --out assets
cargo run -p visioncraft-assets -- check --out assets
```

`fetch` is gentle on the sources (sequential, cached, `--delay-ms`).

## CLI and MCP

```
cargo run -p visioncraft-cli -- commands            # list every command
cargo run -p visioncraft-cli -- mcp --automation-read-root . --automation-write-root .
```

Subcommands include `run`, `batch`, `info`, `render`, `export`, `replay`, `convert`, `new`,
`serve` (JSON lines) and `mcp`. `visioncraft-cli mcp --bridge` talks to a running window.

- [crates/visioncraft-automation/README.md](crates/visioncraft-automation/README.md): layout and
  how to register the MCP server with Claude Code.
- [crates/visioncraft-automation/docs/control-protocol.md](crates/visioncraft-automation/docs/control-protocol.md):
  the protocol, MCP tools, control channel and limits.
- [crates/visioncraft-automation/docs/agent.md](crates/visioncraft-automation/docs/agent.md): guide
  for an agent, with a worked example.

## Checks

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --target wasm32-unknown-unknown -p visioncraft-core -p visioncraft-render -p visioncraft-automation -p visioncraft
cargo deny check
```

CI runs the same ([.github/workflows/ci.yml](.github/workflows/ci.yml)).

## Licence

MIT, see [LICENSE](LICENSE). Assets carry their own licences (CC0 or CC-BY), listed in
[assets/ATTRIBUTION.md](assets/ATTRIBUTION.md).
