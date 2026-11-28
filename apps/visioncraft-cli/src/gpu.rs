//! Pictures without a window: the app's window-free [`Studio`] on a headless wgpu device, made
//! at the first picture and kept for the next.

use glam::Vec3;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use visioncraft::camera::Camera;
use visioncraft::scene::Loaded;
use visioncraft::studio::{Readback, Studio};
use visioncraft_automation::{LoadedModel, Pictures, RenderRequest, Rendered};
use visioncraft_core::{Project, Session};
use visioncraft_render::{Quality, Renderer};

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    studio: Studio,
    /// Model id -> the geometry the studio has for it.
    installed: HashMap<String, Arc<visioncraft_core::ImportedModel>>,
    version: u64,
}

/// The CLI's [`Pictures`]. Without an adapter every picture is an error that says so.
#[derive(Default)]
pub struct GpuPictures {
    gpu: Option<Result<Gpu, String>>,
}

fn encode_png(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .map_err(|e| format!("cannot encode the PNG: {e}"))?;
    Ok(out)
}

impl Pictures for GpuPictures {
    fn render(
        &mut self,
        project: &Project,
        models: &BTreeMap<String, LoadedModel>,
        request: &RenderRequest,
    ) -> Result<Rendered, String> {
        let gpu = self.gpu.get_or_insert_with(|| {
            let (device, queue) = pollster::block_on(visioncraft_render::headless_device())?;
            let renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
            let mut studio = Studio::new(renderer, Project::default());
            // No calibration without a window: a discrete GPU counts as a fast one.
            if visioncraft_render::hardware::HardwareProbe::from_device(&device).kind
                == visioncraft_render::hardware::GpuKind::Discrete
            {
                studio.gpu_lawn_budget = 4.0;
            }
            Ok(Gpu {
                studio,
                device,
                queue,
                installed: HashMap::new(),
                version: 1 << 32,
            })
        });
        let g = gpu
            .as_mut()
            .map_err(|e| format!("no GPU for pictures: {e}"))?;
        let s = &mut g.studio;
        s.set_session(Session::new(project.clone()));
        // Geometry: the host's, installed once per model and version.
        s.loaded.retain(|id, _| models.contains_key(id));
        g.installed.retain(|id, _| models.contains_key(id));
        for (id, m) in models {
            if g.installed
                .get(id)
                .is_none_or(|have| !Arc::ptr_eq(have, &m.model))
            {
                g.version += 1;
                s.loaded.insert(
                    id.clone(),
                    Loaded {
                        model: m.model.clone(),
                        version: g.version,
                    },
                );
                g.installed.insert(id.clone(), m.model.clone());
            }
        }
        // Plan images: the host's bytes, held by the studio's asset store by their file.
        for (file, bytes) in &request.images {
            let store = &mut s.scene.library.store;
            if store
                .provided(file)
                .is_none_or(|held| !Arc::ptr_eq(&held, bytes))
            {
                store.provide(file, bytes.clone());
            }
        }
        // Plants: the host's catalogue, for the sizes the sun-hours map casts crowns from.
        for a in &request.assets {
            if !s.assets.iter().any(|x| x.id == a.id) {
                s.assets.push(a.clone());
            }
        }
        s.set_draft(None);
        s.quality_auto = false;
        s.quality = Quality::PRESETS
            .iter()
            .position(|(name, _)| name.eq_ignore_ascii_case(&request.quality))
            // A photo is taken at ULTRA; an unknown name renders on HIGH (ULTRA is for fast
            // desktop GPUs).
            .unwrap_or(if request.photo.is_some() { 3 } else { 2 });
        s.set_shadow_filter(request.shadow_filter, request.shadow_radius_m);
        s.resize(request.width, request.height);
        s.clock = request.animation_s;
        // What plays, at the picture's clock: the clips on the scene and the flashes.
        s.playback.clone_from(&request.playback);
        s.playback.clock = request.animation_s;
        // What only this picture changes: the lamps' level, the exposure compensation and the
        // grade mixed in (set for every picture, the studio is kept for the next).
        s.lamp_level = request.lamp_level;
        s.exposure = request.exposure_offset_ev;
        s.grade_mix = request.grade_mix;
        // A picture shows its own weather, never the last one's on the way to it.
        s.weather_shown = None;
        s.sync();
        let (position, target, fov) = match request.camera {
            Some((p, t, fov)) => {
                s.camera = Camera::default();
                s.camera.fov_y = fov.to_radians();
                s.camera.look_from(Vec3::from(p), Vec3::from(t));
                (p, t, fov)
            }
            None => {
                s.camera = Camera::default();
                s.frame_all();
                let (eye, pivot) = (s.camera.eye(), s.camera.pivot);
                (
                    eye.to_array(),
                    pivot.to_array(),
                    s.camera.fov_y.to_degrees(),
                )
            }
        };
        // Plants are generated on worker threads and library textures arrive from loader
        // threads: wait for both by the clock (`settle_for_picture`).
        let (frames, problems) = s.settle_for_picture();
        s.problems.extend(problems);
        let pending = s.plants_pending();
        let mut photo_s = None;
        let pixels = match &request.photo {
            Some(p) => {
                let quality = s.current_quality();
                let start = std::time::Instant::now();
                let pixels = s.render_photo(request.width, request.height, quality, p)?;
                photo_s = Some(start.elapsed().as_secs_f64());
                pixels
            }
            None => {
                let rb = Readback::start(s, &g.device, &g.queue);
                g.device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .map_err(|e| format!("the GPU did not finish: {e}"))?;
                rb.take(&g.device)
                    .ok_or("reading the picture back from the GPU failed")?
            }
        };
        let stats = s.renderer.stats();
        let problems = std::mem::take(&mut s.problems);
        for p in &problems {
            eprintln!("visioncraft-cli: {p}");
        }
        Ok(Rendered {
            png: encode_png(&pixels, request.width, request.height)?,
            info: json!({
                "renderer": "gpu",
                "camera": {"position": position, "target": target, "fov_y_deg": fov},
                "quality": request.quality,
                "samples": request.photo.as_ref().map(|p| p.samples),
                // The photo's own time on the GPU, without growing plants and loading textures.
                "photo_s": photo_s,
                "focus_m": request
                    .photo
                    .as_ref()
                    .filter(|p| p.f_number.is_some() || p.aperture_m > 0.0)
                    .and(s.photo_focus_m),
                "frames": frames,
                "plantsPending": pending,
                "problems": problems,
                "lightsDropped": s.dropped_lights,
                // Of the last frame; `gpu_ms` is null when the adapter has no timestamp queries.
                "stats": {
                    "triangles": stats.triangles,
                    "draws": stats.draw_calls,
                    "pass_calls": stats.pass_calls,
                    "instances": stats.instances,
                    "gpu_ms": stats.gpu_ms,
                    "gpu_passes": stats.gpu_passes,
                    // The frame's water surfaces and how each was reflected (`ui.get`).
                    "water": visioncraft::agents::water_json(&stats.water),
                },
            }),
        })
    }
}
