//! VisionCraft test bed: the built-in house scene full-bleed under an egui
//! HUD (time of day, exposure, quality, fps). Drag to orbit, scroll to zoom.

use std::sync::Arc;

use eframe::egui;
use glam::Vec3;
use visioncraft_render::{Quality, Renderer, Sun, demo};

/// Orbit camera around `TARGET`.
const TARGET: Vec3 = Vec3::new(0.0, 1.2, 2.0);

pub fn orbit_camera(yaw: f32, pitch: f32, distance: f32, aspect: f32) -> demo::Camera {
    let eye = TARGET
        + distance
            * Vec3::new(
                yaw.sin() * pitch.cos(),
                pitch.sin(),
                yaw.cos() * pitch.cos(),
            );
    demo::camera(eye, TARGET, aspect)
}

pub struct App {
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    renderer: Renderer,
    texture: Option<egui::TextureId>,
    size: [u32; 2],
    quality: usize,
    azimuth: f32,
    elevation: f32,
    ev100: f32,
    yaw: f32,
    pitch: f32,
    distance: f32,
    auto_orbit: bool,
    last_time: f64,
    window_start: f64,
    window_frames: u32,
    fps: f64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let state = cc
            .wgpu_render_state
            .as_ref()
            .expect("eframe must run on wgpu");
        // egui samples user textures as plain Rgba8Unorm holding sRGB bytes.
        let mut renderer =
            Renderer::new(&state.device, &state.queue, wgpu::TextureFormat::Rgba8Unorm);
        demo::house(&mut renderer);
        renderer.set_overlay_lines(&demo::grid_lines(20, 1.0, [255, 255, 255, 40], 1.0));
        let quality = 1; // medium: the preset that holds 30 fps on the reference laptop GPU
        renderer.set_quality(Quality::PRESETS[quality].1);
        Self {
            device: state.device.clone(),
            queue: state.queue.clone(),
            egui_renderer: state.renderer.clone(),
            renderer,
            texture: None,
            size: [0, 0],
            quality,
            azimuth: 240.0,
            elevation: 18.0,
            ev100: 14.5,
            yaw: 0.6,
            pitch: 0.25,
            distance: 18.0,
            auto_orbit: true,
            last_time: 0.0,
            window_start: 0.0,
            window_frames: 0,
            fps: 0.0,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let rect = ctx.content_rect();
        let ppp = ctx.pixels_per_point();
        let size = [
            (rect.width() * ppp).round().max(1.0) as u32,
            (rect.height() * ppp).round().max(1.0) as u32,
        ];
        let resized = size != self.size;
        if resized {
            self.renderer.resize(size[0], size[1]);
            self.size = size;
        }

        // Camera: drag to orbit, scroll to zoom, slow auto-orbit otherwise.
        let time = ctx.input(|i| i.time);
        let dt = (time - self.last_time).clamp(0.0, 0.1) as f32;
        self.last_time = time;
        let response = ui.interact(rect, egui::Id::new("viewport"), egui::Sense::drag());
        let drag = response.drag_delta();
        if drag != egui::Vec2::ZERO {
            self.auto_orbit = false;
            self.yaw -= drag.x * 0.006;
            self.pitch = (self.pitch + drag.y * 0.004).clamp(0.02, 1.4);
        } else if self.auto_orbit {
            self.yaw += dt * 0.1;
        }
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        self.distance = (self.distance * (1.0 - scroll * 0.002)).clamp(3.0, 120.0);
        let (view, proj, eye) = orbit_camera(
            self.yaw,
            self.pitch,
            self.distance,
            size[0] as f32 / size[1] as f32,
        );
        self.renderer.set_camera(view, proj, eye);
        self.renderer
            .set_sun(&Sun::from_angles(self.azimuth, self.elevation));
        self.renderer.set_exposure(self.ev100);
        let rebuilt = resized || Quality::PRESETS[self.quality].1 != self.renderer.quality();
        self.renderer.set_quality(Quality::PRESETS[self.quality].1);

        let mut encoder = self.device.create_command_encoder(&Default::default());
        let view = self.renderer.render(&mut encoder);
        self.queue.submit([encoder.finish()]);
        // The output texture is new after a resize or quality change: re-register.
        let mut egui_renderer = self.egui_renderer.write();
        let id = match self.texture {
            Some(id) => {
                if rebuilt {
                    egui_renderer.update_egui_texture_from_wgpu_texture(
                        &self.device,
                        view,
                        wgpu::FilterMode::Linear,
                        id,
                    );
                }
                id
            }
            None => {
                egui_renderer.register_native_texture(&self.device, view, wgpu::FilterMode::Linear)
            }
        };
        drop(egui_renderer);
        self.texture = Some(id);
        ui.painter().image(
            id,
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        // FPS and frame time averaged over the last second.
        self.window_frames += 1;
        if time - self.window_start >= 1.0 {
            self.fps = f64::from(self.window_frames) / (time - self.window_start);
            self.window_start = time;
            self.window_frames = 0;
        }
        egui::Area::new(egui::Id::new("hud"))
            .fixed_pos(egui::pos2(10.0, 10.0))
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(format!(
                        "{:.1} fps, {:.2} ms",
                        self.fps,
                        1000.0 / self.fps.max(1e-6)
                    ));
                    ui.add(
                        egui::Slider::new(&mut self.elevation, -10.0..=90.0).text("sun elevation"),
                    );
                    ui.add(egui::Slider::new(&mut self.azimuth, 0.0..=360.0).text("sun azimuth"));
                    ui.add(egui::Slider::new(&mut self.ev100, 8.0..=17.0).text("exposure (EV100)"));
                    egui::ComboBox::from_label("quality")
                        .selected_text(Quality::PRESETS[self.quality].0)
                        .show_ui(ui, |ui| {
                            for (i, (name, _)) in Quality::PRESETS.iter().enumerate() {
                                ui.selectable_value(&mut self.quality, i, *name);
                            }
                        });
                    ui.checkbox(&mut self.auto_orbit, "auto orbit");
                });
            });
        ctx.request_repaint();
    }
}
