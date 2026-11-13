//! Orbit, walk and top views: the maths only, no UI. Y up, yaw/pitch in radians.

use glam::{Mat4, Vec2, Vec3};
use visioncraft_core::{Viewpoint, interpolate};

pub const EYE_HEIGHT: f32 = 1.7;
pub const WALK_SPEED: f32 = 1.4;
/// The top view is a very narrow perspective from far above: the renderer's shadow maths needs a
/// perspective projection, and at this angle it is parallel for all practical purposes.
const TOP_FOV: f32 = 0.105;
pub const FLY_SECONDS: f64 = 1.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Orbit,
    Walk,
    Top,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Walk {
    pub eye: Vec3,
    /// Compass heading: 0 looks north (-Z), clockwise.
    pub yaw: f32,
    pub pitch: f32,
    /// Height of the ground under the feet at the last successful query.
    pub ground: f32,
}

#[derive(Clone, Debug)]
struct Fly {
    from: Viewpoint,
    to: Viewpoint,
    start: f64,
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub view: View,
    pub pivot: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub fov_y: f32,
    pub walk: Walk,
    pub top_center: Vec2,
    pub top_half_height: f32,
    fly: Option<Fly>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            view: View::Orbit,
            pivot: Vec3::new(0.0, 1.0, 0.0),
            yaw: 0.6,
            pitch: 0.4,
            dist: 20.0,
            fov_y: 50f32.to_radians(),
            walk: Walk {
                eye: Vec3::new(0.0, EYE_HEIGHT, 10.0),
                yaw: 0.0,
                pitch: 0.0,
                ground: 0.0,
            },
            top_center: Vec2::ZERO,
            top_half_height: 15.0,
            fly: None,
        }
    }
}

pub fn orbit_eye(pivot: Vec3, yaw: f32, pitch: f32, dist: f32) -> Vec3 {
    pivot
        + dist
            * Vec3::new(
                yaw.sin() * pitch.cos(),
                pitch.sin(),
                yaw.cos() * pitch.cos(),
            )
}

pub fn walk_forward(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
}

/// New ground position after `dt` seconds with `fwd`/`right` in -1..1.
pub fn walk_step(xz: Vec2, yaw: f32, fwd: f32, right: f32, speed: f32, dt: f32) -> Vec2 {
    let f = Vec2::new(yaw.sin(), -yaw.cos());
    let r = Vec2::new(yaw.cos(), yaw.sin());
    let v = f * fwd + r * right;
    let v = if v.length() > 1.0 { v.normalize() } else { v };
    xz + v * speed * dt
}

/// Ray through `pos` (points from the viewport's top-left) of a viewport `size` points large.
pub fn ray_through(view: Mat4, proj: Mat4, pos: Vec2, size: Vec2) -> (Vec3, Vec3) {
    let ndc = Vec2::new(pos.x / size.x * 2.0 - 1.0, 1.0 - pos.y / size.y * 2.0);
    let inv = (proj * view).inverse();
    let near = inv.project_point3(ndc.extend(0.0));
    let far = inv.project_point3(ndc.extend(1.0));
    (near, (far - near).normalize())
}

/// Where a ray meets the horizontal plane `y`.
pub fn ray_plane(origin: Vec3, dir: Vec3, y: f32) -> Option<Vec3> {
    if dir.y.abs() < 1e-6 {
        return None;
    }
    let t = (y - origin.y) / dir.y;
    (t > 0.0).then(|| origin + dir * t)
}

impl Camera {
    pub fn eye(&self) -> Vec3 {
        match self.view {
            View::Orbit => orbit_eye(self.pivot, self.yaw, self.pitch, self.dist),
            View::Walk => self.walk.eye,
            View::Top => self.top_matrices(1.0).2,
        }
    }

    fn top_distance(&self) -> f32 {
        self.top_half_height / (TOP_FOV / 2.0).tan()
    }

    fn top_matrices(&self, aspect: f32) -> (Mat4, Mat4, Vec3) {
        let d = self.top_distance();
        let c = Vec3::new(self.top_center.x, 0.0, self.top_center.y);
        let eye = c + Vec3::Y * d;
        (
            Mat4::look_at_rh(eye, c, Vec3::NEG_Z),
            Mat4::perspective_rh(TOP_FOV, aspect, d * 0.3, d * 3.0),
            eye,
        )
    }

    /// (view, projection, eye)
    pub fn matrices(&self, aspect: f32) -> (Mat4, Mat4, Vec3) {
        match self.view {
            View::Top => self.top_matrices(aspect),
            View::Orbit => {
                let eye = self.eye();
                let near = (self.dist * 0.02).clamp(0.005, 5.0);
                (
                    Mat4::look_at_rh(eye, self.pivot, Vec3::Y),
                    Mat4::perspective_rh(self.fov_y, aspect, near, 3000.0),
                    eye,
                )
            }
            View::Walk => {
                let w = self.walk;
                (
                    Mat4::look_at_rh(w.eye, w.eye + walk_forward(w.yaw, w.pitch), Vec3::Y),
                    Mat4::perspective_rh(self.fov_y, aspect, 0.1, 3000.0),
                    w.eye,
                )
            }
        }
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        match self.view {
            View::Walk => {
                self.walk.yaw += dx * 0.004;
                self.walk.pitch = (self.walk.pitch - dy * 0.004).clamp(-1.4, 1.4);
            }
            View::Orbit => {
                self.yaw -= dx * 0.006;
                self.pitch = (self.pitch + dy * 0.004).clamp(-0.2, 1.5);
            }
            View::Top => {}
        }
    }

    /// Pan by a pointer movement in points; `viewport_h` is the viewport height in points.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        match self.view {
            View::Top => {
                let k = 2.0 * self.top_half_height / viewport_h;
                self.top_center += Vec2::new(-dx, -dy) * k;
            }
            View::Orbit => {
                let k = 2.0 * self.dist * (self.fov_y / 2.0).tan() / viewport_h;
                let fwd = (self.pivot - self.eye()).normalize();
                let right = fwd.cross(Vec3::Y).normalize_or_zero();
                let up = right.cross(fwd);
                self.pivot += (-right * dx + up * dy) * k;
            }
            View::Walk => {}
        }
    }

    pub fn zoom(&mut self, scroll: f32) {
        let f = (1.0 - scroll * 0.002).clamp(0.5, 2.0);
        match self.view {
            View::Orbit => self.dist = (self.dist * f).clamp(0.05, 2000.0),
            View::Top => self.top_half_height = (self.top_half_height * f).clamp(1.0, 500.0),
            View::Walk => {}
        }
    }

    /// Keeps the eye where it is and aims the orbit at `p`.
    pub fn set_pivot(&mut self, p: Vec3) {
        let eye = self.eye();
        self.look_from(eye, p);
    }

    /// Orbit view with the eye at `pos`, aiming at `target`.
    pub fn look_from(&mut self, pos: Vec3, target: Vec3) {
        let d = pos - target;
        self.view = View::Orbit;
        self.pivot = target;
        self.dist = d.length().max(0.05);
        self.yaw = d.x.atan2(d.z);
        self.pitch = (d.y / self.dist).clamp(-1.0, 1.0).asin();
    }

    /// Fits the box `[min, max]` in view.
    pub fn frame(&mut self, min: Vec3, max: Vec3) {
        let centre = (min + max) / 2.0;
        let radius = ((max - min).length() / 2.0).max(0.02);
        self.view = View::Orbit;
        self.pivot = centre;
        self.dist = radius / (self.fov_y / 2.0).tan() * 0.7;
        self.top_center = Vec2::new(centre.x, centre.z);
        self.top_half_height = (radius * 1.1).max(1.0);
    }

    pub fn enter_walk(&mut self, ground: Option<f32>) {
        if self.view == View::Walk {
            return;
        }
        let eye = self.eye();
        let to = self.pivot - eye;
        let g = ground.unwrap_or(0.0);
        self.walk = Walk {
            eye: Vec3::new(eye.x, g + EYE_HEIGHT, eye.z),
            yaw: if Vec2::new(to.x, to.z).length() > 1e-3 {
                to.x.atan2(-to.z)
            } else {
                self.walk.yaw
            },
            pitch: -0.05,
            ground: g,
        };
        self.view = View::Walk;
    }

    pub fn leave_walk(&mut self) {
        if self.view == View::Walk {
            let w = self.walk;
            let f = walk_forward(w.yaw, 0.0);
            self.look_from(
                w.eye - f * 6.0 + Vec3::Y * 3.0,
                w.eye + f * 4.0 - Vec3::Y * (EYE_HEIGHT - 0.5),
            );
        }
    }

    /// One frame of walking. `ground` returns the floor height below `(x, from_y, z)`.
    pub fn walk_update(
        &mut self,
        dt: f32,
        fwd: f32,
        right: f32,
        run: bool,
        ground: &dyn Fn(f32, f32, f32) -> Option<f32>,
    ) {
        let w = &mut self.walk;
        let speed = if run { WALK_SPEED * 3.0 } else { WALK_SPEED };
        let xz = walk_step(Vec2::new(w.eye.x, w.eye.z), w.yaw, fwd, right, speed, dt);
        w.eye.x = xz.x;
        w.eye.z = xz.y;
        // Cast from a step's height above the feet: steps up to 0.6 m are climbed, higher
        // ceilings (a roof above the head) are not mistaken for the floor.
        if let Some(g) = ground(xz.x, w.ground + 0.6, xz.y) {
            w.ground = g;
        } // else: no ground here, keep the last height
        let target = w.ground + EYE_HEIGHT;
        w.eye.y += (target - w.eye.y) * (1.0 - (-12.0 * dt).exp());
    }

    pub fn is_flying(&self) -> bool {
        self.fly.is_some()
    }

    /// Current position and target, as a viewpoint would store them.
    pub fn pose(&self) -> (Vec3, Vec3) {
        match self.view {
            View::Walk => (
                self.walk.eye,
                self.walk.eye + walk_forward(self.walk.yaw, self.walk.pitch) * 5.0,
            ),
            _ => (self.eye(), self.pivot),
        }
    }

    pub fn start_fly(&mut self, to: &Viewpoint, now: f64) {
        let (p, t) = self.pose();
        let from = Viewpoint {
            id: String::new(),
            name: String::new(),
            position: p.to_array(),
            target: t.to_array(),
            fov_y_deg: self.fov_y.to_degrees(),
            time: None,
        };
        if self.view != View::Orbit {
            self.look_from(p, t);
        }
        self.fly = Some(Fly {
            from,
            to: to.clone(),
            start: now,
        });
    }

    /// Advances a running fly-to; false when none is running.
    pub fn fly_update(&mut self, now: f64) -> bool {
        let Some(f) = &self.fly else { return false };
        let t = ((now - f.start) / FLY_SECONDS) as f32;
        let (p, tg) = interpolate(&f.from, &f.to, t);
        let fov = f.from.fov_y_deg + (f.to.fov_y_deg - f.from.fov_y_deg) * t.clamp(0.0, 1.0);
        self.look_from(Vec3::from(p), Vec3::from(tg));
        self.fov_y = fov.to_radians();
        if t >= 1.0 {
            self.fly = None;
        }
        true
    }

    pub fn cancel_fly(&mut self) {
        self.fly = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_from_round_trips_the_eye() {
        let mut c = Camera::default();
        let (pos, target) = (Vec3::new(5.0, 3.0, -7.0), Vec3::new(1.0, 1.0, 1.0));
        c.look_from(pos, target);
        assert!((c.eye() - pos).length() < 1e-4);
    }

    #[test]
    fn walking_north_decreases_z_and_is_capped_when_diagonal() {
        let p = walk_step(Vec2::ZERO, 0.0, 1.0, 0.0, 1.4, 1.0);
        assert!((p - Vec2::new(0.0, -1.4)).length() < 1e-5);
        let d = walk_step(Vec2::ZERO, 0.0, 1.0, 1.0, 1.4, 1.0);
        assert!((d.length() - 1.4).abs() < 1e-5);
        let east = walk_step(Vec2::ZERO, 0.0, 0.0, 1.0, 1.4, 1.0);
        assert!(east.x > 1.3);
    }

    #[test]
    fn centre_ray_hits_the_pivot_plane() {
        let mut c = Camera::default();
        c.look_from(Vec3::new(0.0, 10.0, 10.0), Vec3::ZERO);
        let (v, p, _) = c.matrices(2.0);
        let (o, d) = ray_through(v, p, Vec2::new(100.0, 50.0), Vec2::new(200.0, 100.0));
        let hit = ray_plane(o, d, 0.0).unwrap();
        assert!(hit.length() < 1e-2, "{hit}");
    }

    #[test]
    fn top_view_looks_straight_down_with_north_up() {
        let c = Camera {
            view: View::Top,
            ..Camera::default()
        };
        let (v, p, _) = c.matrices(1.0);
        let north = (p * v).project_point3(Vec3::new(0.0, 0.0, -5.0));
        let east = (p * v).project_point3(Vec3::new(5.0, 0.0, 0.0));
        assert!(north.y > 0.0 && east.x > 0.0);
    }

    #[test]
    fn walk_keeps_last_ground_when_none() {
        let mut c = Camera::default();
        c.walk.ground = 2.0;
        c.walk.eye = Vec3::new(0.0, 2.0 + EYE_HEIGHT, 0.0);
        c.view = View::Walk;
        c.walk_update(0.1, 1.0, 0.0, false, &|_, _, _| None);
        assert_eq!(c.walk.ground, 2.0);
        c.walk_update(0.1, 0.0, 0.0, false, &|_, _, _| Some(2.3));
        assert_eq!(c.walk.ground, 2.3);
    }
}
