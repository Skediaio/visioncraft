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
            Ok(Gpu {
                studio: Studio::new(renderer, Project::default()),
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
        s.session = Session::new(project.clone());
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
        s.set_draft(None);
        s.quality_auto = false;
        s.quality = Quality::PRESETS
            .iter()
            .position(|(name, _)| name.eq_ignore_ascii_case(&request.quality))
            // An unknown name renders on HIGH (ULTRA is for fast desktop GPUs).
            .unwrap_or(2);
        s.resize(request.width, request.height);
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
        // Plants are generated a step per frame: let them finish (as smoke.rs does).
        let mut frames = 0;
        for _ in 0..600 {
            s.prepare();
            frames += 1;
            if s.plants_pending() == 0 {
                break;
            }
        }
        s.prepare();
        let pending = s.plants_pending();
        let rb = Readback::start(s, &g.device, &g.queue);
        g.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("the GPU did not finish: {e}"))?;
        let pixels = rb
            .take(&g.device)
            .ok_or("reading the picture back from the GPU failed")?;
        let problems = std::mem::take(&mut s.problems);
        Ok(Rendered {
            png: encode_png(&pixels, request.width, request.height)?,
            info: json!({
                "renderer": "gpu",
                "camera": {"position": position, "target": target, "fov_y_deg": fov},
                "quality": request.quality,
                "frames": frames,
                "plantsPending": pending,
                "problems": problems,
                "lightsDropped": s.dropped_lights,
            }),
        })
    }
}
