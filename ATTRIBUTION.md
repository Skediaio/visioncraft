# Code attribution

What the source comments of this repository say about where code and methods come from. Assets
are covered by [assets/ATTRIBUTION.md](assets/ATTRIBUTION.md). VisionCraft is MIT; nothing here is
copied from closed software. Papers and articles are cited for the published method; the code is
written here unless the licence column says otherwise.

## From 3dcraft (MIT, <https://github.com/johancoppens/3dcraft>)

| Where | Origin | Licence |
|---|---|---|
| `crates/visioncraft-core/Cargo.toml` | `craft3d-format`, `-doc`, `-regen`, `-tess`, `-topo`, `-zipstore` as git dependencies, tag `v0.1.1` | MIT |
| `crates/visioncraft-automation/Cargo.toml` | `craft3d-doc` (SHA-256) as git dependency | MIT |
| `crates/visioncraft-core/src/import.rs` | Ported in outline from `crates/engine/src/scene.rs` | MIT |
| `crates/visioncraft-automation/src/fields.rs` | Ported from `crates/automation/src/fields.rs` | MIT |
| `crates/visioncraft-automation/src/lines.rs` | Ported from `crates/automation/src/lines.rs` | MIT |
| `crates/visioncraft-automation/src/rpc.rs` | Ported from `crates/automation/src/rpc.rs` | MIT |
| `crates/visioncraft-automation/src/proof.rs` | Ported from `crates/automation/src/proof.rs` | MIT |
| `crates/visioncraft-automation/src/discovery.rs` | Ported from `crates/automation/src/discovery.rs` | MIT |
| `crates/visioncraft-automation/src/bridge.rs` | Ported from `crates/automation/src/bridge.rs` | MIT |
| `crates/visioncraft-automation/src/lib.rs`, `control.rs` | Modelled on `craft3d-automation` v0.1.1 and its `docs/control-protocol.md` | MIT |
| `apps/visioncraft-cli/src/main.rs` | Shaped after `craft3d-cli` | MIT |

## From other open-source code

| Where | Origin | Licence |
|---|---|---|
| `crates/visioncraft-render/src/tonemap.wgsl`, `tonemap.rs` | AgX by Troy Sobotka, in the formulation of three.js `AgXToneMapping` (ShaderChunk `tonemapping_pars_fragment`), which ports Filament's AgX (google/filament PR 7236); 6th-order sigmoid fit from Benjamin Wrensch, "Minimal AgX Implementation" (2023) | three.js: MIT; Filament: Apache-2.0 |
| `crates/visioncraft-render/src/post.wgsl` | Sharpening as in AMD FidelityFX FSR 1 "RCAS" (`ffx_fsr1.h`) | MIT (as stated in the comment) |

## Papers and articles (methods, as cited in the comments)

| Where | Cited work |
|---|---|
| `crates/visioncraft-core/src/sun.rs` | NOAA Global Monitoring Laboratory, "General Solar Position Calculations" and its solar calculator spreadsheet (after Meeus, *Astronomical Algorithms*) |
| `crates/visioncraft-render/src/sky.rs`, `sky.wgsl` | Preetham, Shirley & Smits, "A Practical Analytic Model for Daylight", SIGGRAPH 1999 (Appendix A); Perez et al. 1993 distribution |
| `crates/visioncraft-render/src/sky.wgsl` | Hillaire, "A Scalable and Production Ready Sky and Atmosphere Rendering Technique", EGSR 2020 |
| `crates/visioncraft-render/src/sky.rs` | Kasten & Young, "Revised optical air mass tables and approximation formula" (air mass); Filament documentation, "Physically based units" |
| `crates/visioncraft-render/src/lights.rs` | Filament documentation, "Physically based rendering in Filament", section 5.2.2 |
| `crates/visioncraft-render/src/common.wgsl` | Lagarde & de Rousiers, "Moving Frostbite to PBR", 2014 (pre-exposure); Jimenez, "Next Generation Post Processing in Call of Duty: Advanced Warfare", SIGGRAPH 2014 (interleaved gradient noise); Jarzynski & Olano, "Hash Functions for GPU Rendering", JCGT 2020 (PCG); Wenzel, "Real-time Atmospheric Effects in Games Revisited", GDC 2007 and Quilez, "Better Fog", 2010 (height fog) |
| `crates/visioncraft-render/src/env.wgsl` | Karis, "Real Shading in Unreal Engine 4", SIGGRAPH 2013; Krivanek & Colbert, "Real-time Shading with Filtered Importance Sampling", EGSR 2008; Ramamoorthi & Hanrahan, "An Efficient Representation for Irradiance Environment Maps", SIGGRAPH 2001 |
| `crates/visioncraft-render/src/scene.wgsl` | Sousa, "Vegetation Procedural Animation and Shading in Crysis", GPU Gems 3 ch. 16; Golus, "Anti-aliased Alpha Test" (2017) and "Normal Mapping for a Triplanar Shader" (2017); Ramamoorthi & Hanrahan 2001 (L2 SH irradiance); Karis, "Physically Based Shading on Mobile" (2014) and UE4 notes 2013 (falloff, eq. 9); Holbert, "Saying Goodbye to Shadow Acne", GDC 2011; Kopf et al., "Joint Bilateral Upsampling", SIGGRAPH 2007; Schueler, "Normal Mapping without Precomputed Tangents", ShaderX5 (2006); Barre-Brisebois & Hill, "Blending in Detail" (2012); Lengyel, "Foundations of Game Engine Development" vol. 1 (2016) |
| `crates/visioncraft-render/src/shadow.rs` | Zhang et al., "Parallel-Split Shadow Maps for Large-scale Virtual Environments", VRCIA 2006; Valient, "Stable Rendering of Cascaded Shadow Maps", ShaderX6 (2008); Microsoft, "Common Techniques to Improve Shadow Depth Maps" |
| `crates/visioncraft-render/src/ssao.wgsl` | Bavoil & Sainz, "Screen Space Ambient Occlusion", NVIDIA 2008; Yuwen Wu, "Accurate Normal Reconstruction from Depth Buffer" (2021); Karabelas, "Screen space shadows" (2017) |
| `crates/visioncraft-render/src/blur.wgsl` | Bilateral filtering (ICCV 1998), as used for SSAO |
| `crates/visioncraft-render/src/post.wgsl` | Jimenez 2014 (bloom); Lottes, "FXAA", NVIDIA whitepaper 2009 |
| `crates/visioncraft-render/src/terrain.wgsl` | Mishkinis, "Advanced Terrain Texture Splatting" (Gamasutra, 2013) |
