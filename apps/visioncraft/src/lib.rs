//! VisionCraft spike app: the renderer's output full-bleed, an egui overlay on top.

use std::sync::Arc;

use eframe::egui;
use glam::{Mat4, Vec3};
use visioncraft_render::Renderer;

const SPACING: f32 = 2.5;

/// `n` cubes on a square grid centred on the origin, heights from a hash.
pub fn grid(n: usize) -> Vec<Mat4> {
    let side = (n as f32).sqrt().ceil().max(1.0) as usize;
    let half = side as f32 / 2.0;
    (0..n)
        .map(|i| {
            let (x, z) = ((i % side) as f32 - half, (i / side) as f32 - half);
            let h = 1.0 + ((i as u32).wrapping_mul(2654435761) >> 28) as f32 * 0.4;
            Mat4::from_translation(Vec3::new(x * SPACING, h / 2.0, z * SPACING))
                * Mat4::from_scale(Vec3::new(1.0, h, 1.0))
        })
        .collect()
}

pub fn view_proj(angle: f32, count: usize, aspect: f32) -> Mat4 {
    let radius = (count as f32).sqrt() * SPACING * 0.6 + 25.0;
    let eye = Vec3::new(angle.cos() * radius, radius * 0.55, angle.sin() * radius);
    Mat4::perspective_rh(55f32.to_radians(), aspect, 0.1, 5000.0)
        * Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y)
}

pub struct App {
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui_renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    renderer: Renderer,
    texture: Option<egui::TextureId>,
    size: [u32; 2],
    count: usize,
    uploaded: usize,
    window_start: f64,
    window_frames: u32,
    fps: f64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let state = cc.wgpu_render_state.as_ref().expect("eframe must run on wgpu");
        let renderer = Renderer::new(&state.device, &state.queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        Self {
            device: state.device.clone(),
            queue: state.queue.clone(),
            egui_renderer: state.renderer.clone(),
            renderer,
            texture: None,
            size: [0, 0],
            count: 5000,
            uploaded: usize::MAX,
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
        let size = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
        let resized = size != self.size;
        if resized {
            self.renderer.resize(size[0], size[1]);
            self.size = size;
        }
        if self.uploaded != self.count {
            self.renderer.set_instances(&grid(self.count));
            self.uploaded = self.count;
        }
        let time = ctx.input(|i| i.time);
        self.renderer.set_camera(view_proj(time as f32 * 0.15, self.count, size[0] as f32 / size[1] as f32));

        let mut encoder = self.device.create_command_encoder(&Default::default());
        let view = self.renderer.render(&mut encoder);
        self.queue.submit([encoder.finish()]);
        // The output texture is new after a resize: (re)register its view.
        let mut egui_renderer = self.egui_renderer.write();
        let id = match self.texture {
            Some(id) => {
                if resized {
                    egui_renderer.update_egui_texture_from_wgpu_texture(&self.device, view, wgpu::FilterMode::Linear, id);
                }
                id
            }
            None => egui_renderer.register_native_texture(&self.device, view, wgpu::FilterMode::Linear),
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
        egui::Area::new(egui::Id::new("hud")).fixed_pos(egui::pos2(10.0, 10.0)).show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(format!("{:.1} fps, {:.2} ms", self.fps, 1000.0 / self.fps.max(1e-6)));
                ui.add(egui::Slider::new(&mut self.count, 100..=50_000).logarithmic(true).text("cubes"));
            });
        });
        ctx.request_repaint();
    }
}
