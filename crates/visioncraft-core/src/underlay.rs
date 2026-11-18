//! A plan under the drawing: a scaled image (a floor plan, a site plan) to trace over.
//!
//! Conventions, fixed here once:
//! - The image's pixel `(0, 0)` is its top-left corner; pixel x runs right along the image's
//!   top edge, pixel y runs down its left edge. Pixel coordinates are continuous: `(0, 0)` is the
//!   corner of the first pixel, `(w, h)` the far corner of the last one.
//! - At rotation 0 image +x is world +X and image +y is world +Z (the drawing is seen from above
//!   with -Z up, so the image reads the right way up and is not mirrored).
//! - `rotation_deg` turns the image about `origin`, counter-clockwise seen from above (+Y up),
//!   as `Transform::rotation_y_deg` does: a point at `(x, z)` relative to the origin moves to
//!   `(x cos r + z sin r, -x sin r + z cos r)`. At +90 the image's +x edge points to -Z (north).

use crate::project::Extensions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The layer an underlay goes on when none is named; created by `underlay.add` when absent.
pub const LAYER_UNDERLAY: &str = "underlay";

/// The largest underlay image file, in bytes.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// The largest side of an underlay image, in pixels.
pub const MAX_SIDE_PX: u32 = 16384;

fn default_opacity() -> f32 {
    0.5
}

fn default_elevation() -> f32 {
    0.02
}

/// Where an image lies on the ground.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// `[x, z]` in metres where the image's top-left corner lands.
    pub origin: [f32; 2],
    pub metres_per_pixel: f32,
    /// Counter-clockwise seen from above, degrees.
    #[serde(default)]
    pub rotation_deg: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            origin: [0.0, 0.0],
            metres_per_pixel: 0.05,
            rotation_deg: 0.0,
        }
    }
}

/// The turn of `(x, z)` counter-clockwise seen from above, radians.
fn turn_of(x: f64, z: f64) -> f64 {
    (-z).atan2(x)
}

impl Placement {
    /// The ground point `[x, z]` under an image pixel (continuous, may lie outside the image).
    pub fn pixel_to_world(&self, pixel: [f32; 2]) -> [f32; 2] {
        let (s, c) = f64::from(self.rotation_deg).to_radians().sin_cos();
        let m = f64::from(self.metres_per_pixel);
        let (x, z) = (f64::from(pixel[0]) * m, f64::from(pixel[1]) * m);
        [
            (f64::from(self.origin[0]) + c * x + s * z) as f32,
            (f64::from(self.origin[1]) - s * x + c * z) as f32,
        ]
    }

    /// The image pixel under a ground point `[x, z]`: the inverse of [`Self::pixel_to_world`].
    pub fn world_to_pixel(&self, world: [f32; 2]) -> [f32; 2] {
        let (s, c) = f64::from(self.rotation_deg).to_radians().sin_cos();
        let m = f64::from(self.metres_per_pixel).max(f64::MIN_POSITIVE);
        let (dx, dz) = (
            f64::from(world[0]) - f64::from(self.origin[0]),
            f64::from(world[1]) - f64::from(self.origin[1]),
        );
        [
            ((c * dx - s * dz) / m) as f32,
            ((s * dx + c * dz) / m) as f32,
        ]
    }

    /// The placement that puts `pixel_a` on `world_a` and `pixel_b` on `world_b`: scale, turn and
    /// origin follow from the two pairs (the image is never mirrored or skewed).
    pub fn calibrated(
        pixel_a: [f32; 2],
        pixel_b: [f32; 2],
        world_a: [f32; 2],
        world_b: [f32; 2],
    ) -> Result<Self, String> {
        let (px, pz) = (
            f64::from(pixel_b[0]) - f64::from(pixel_a[0]),
            f64::from(pixel_b[1]) - f64::from(pixel_a[1]),
        );
        let (wx, wz) = (
            f64::from(world_b[0]) - f64::from(world_a[0]),
            f64::from(world_b[1]) - f64::from(world_a[1]),
        );
        let (pixel_len, world_len) = (px.hypot(pz), wx.hypot(wz));
        if pixel_len.is_nan() || pixel_len <= 1e-6 {
            return Err("the two pixel positions must differ".into());
        }
        if world_len.is_nan() || world_len <= 1e-6 {
            return Err("the two drawing points must differ".into());
        }
        let mpp = world_len / pixel_len;
        let rotation = (turn_of(wx, wz) - turn_of(px, pz)).to_degrees();
        let rotation = rotation - 360.0 * (rotation / 360.0).round();
        let rotation = if rotation <= -180.0 { 180.0 } else { rotation };
        let mut placement = Self {
            origin: [0.0, 0.0],
            metres_per_pixel: mpp as f32,
            rotation_deg: rotation as f32,
        };
        // Where pixel A lands with the origin at zero; the origin is what is missing.
        let at = placement.pixel_to_world(pixel_a);
        placement.origin = [world_a[0] - at[0], world_a[1] - at[1]];
        Ok(placement)
    }

    /// The same turn and origin with the scale that makes two pixels `distance_m` apart.
    pub fn scaled_by_distance(
        &self,
        pixel_a: [f32; 2],
        pixel_b: [f32; 2],
        distance_m: f32,
    ) -> Result<Self, String> {
        let pixels = f64::from(pixel_b[0] - pixel_a[0]).hypot(f64::from(pixel_b[1] - pixel_a[1]));
        if pixels.is_nan() || pixels <= 1e-6 {
            return Err("the two pixel positions must differ".into());
        }
        if !(distance_m.is_finite() && distance_m > 0.0) {
            return Err("distance_m must be above zero".into());
        }
        Ok(Self {
            metres_per_pixel: (f64::from(distance_m) / pixels) as f32,
            ..*self
        })
    }
}

/// A plan image placed in the drawing. It is drawn flat, unlit and see-through, casts no shadow
/// and is not picked; it stays out of the quantities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Underlay {
    /// `u1`, `u2`...
    pub id: String,
    pub name: String,
    /// A path relative to the project file, or `underlays/<name>` inside a bundle.
    pub file: String,
    pub layer: String,
    pub placement: Placement,
    /// Width and height of the image, recorded at import so bounds need no decoding.
    pub size_px: [u32; 2],
    /// 0 (invisible) to 1 (opaque).
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// Height above the ground plane, metres: a little above, so it does not flicker with it.
    #[serde(default = "default_elevation")]
    pub elevation: f32,
    /// Data of other tools by namespace, kept untouched.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: Extensions,
}

impl Underlay {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.file.trim().is_empty() {
            return Err("an underlay needs a name and a file".into());
        }
        let [w, h] = self.size_px;
        if w == 0 || h == 0 || w > MAX_SIDE_PX || h > MAX_SIDE_PX {
            return Err(format!(
                "size_px must be two whole numbers from 1 to {MAX_SIDE_PX}"
            ));
        }
        let p = &self.placement;
        if !p.origin.iter().all(|c| c.is_finite() && c.abs() < 1e6) {
            return Err("origin must be two finite numbers".into());
        }
        if !(p.metres_per_pixel.is_finite() && p.metres_per_pixel > 0.0) {
            return Err("metres_per_pixel must be above zero".into());
        }
        if p.metres_per_pixel * w.max(h) as f32 > 100_000.0 {
            return Err("the image would be more than 100 km across".into());
        }
        if !p.rotation_deg.is_finite() {
            return Err("rotation_deg must be a number".into());
        }
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err("opacity must be between 0 and 1".into());
        }
        if !(self.elevation.is_finite() && self.elevation.abs() <= 100.0) {
            return Err("elevation must be a number between -100 and 100 m".into());
        }
        Ok(())
    }

    /// The image's corners on the ground, `[x, z]`: top-left, top-right, bottom-right,
    /// bottom-left.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let (w, h) = (self.size_px[0] as f32, self.size_px[1] as f32);
        [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(|p| self.placement.pixel_to_world(p))
    }

    /// The smallest `[x, z]` box round the image: `[[min_x, min_z], [max_x, max_z]]`.
    pub fn bounds(&self) -> [[f32; 2]; 2] {
        let c = self.corners();
        let fold = |f: fn(f32, f32) -> f32, i: usize, start: f32| {
            c.iter().fold(start, |acc, p| f(acc, p[i]))
        };
        [
            [
                fold(f32::min, 0, f32::INFINITY),
                fold(f32::min, 1, f32::INFINITY),
            ],
            [
                fold(f32::max, 0, f32::NEG_INFINITY),
                fold(f32::max, 1, f32::NEG_INFINITY),
            ],
        ]
    }
}

/// The width and height of a PNG or JPEG from its header, without decoding it: a message for
/// anything else (never a panic, whatever the bytes).
pub fn image_size(bytes: &[u8]) -> Result<[u32; 2], String> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    let size = if bytes.starts_with(PNG) {
        // 8 bytes of signature, then the IHDR chunk: length, "IHDR", width, height.
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return Err("the PNG is cut short".into());
        }
        let n = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        [n(16), n(20)]
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg_size(bytes).ok_or("the JPEG has no readable size (cut short or damaged)")?
    } else {
        return Err("not a PNG or JPEG image (only those are read)".into());
    };
    if size[0] == 0 || size[1] == 0 {
        return Err("the image is empty".into());
    }
    if size[0] > MAX_SIDE_PX || size[1] > MAX_SIDE_PX {
        return Err(format!(
            "the image is {} x {} pixels; at most {MAX_SIDE_PX} on a side",
            size[0], size[1]
        ));
    }
    Ok(size)
}

/// Walks the JPEG's segments to its first start-of-frame marker.
fn jpeg_size(b: &[u8]) -> Option<[u32; 2]> {
    let mut at = 2;
    loop {
        // skip fill bytes up to a marker
        while *b.get(at)? != 0xFF {
            at += 1;
        }
        while *b.get(at)? == 0xFF {
            at += 1;
        }
        let marker = *b.get(at)?;
        at += 1;
        match marker {
            // no length: stand-alone markers
            0x01 | 0xD0..=0xD8 => continue,
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]));
        // SOF0..SOF15 except DHT (C4), JPG (C8) and DAC (CC)
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([*b.get(at + 3)?, *b.get(at + 4)?]);
            let w = u16::from_be_bytes([*b.get(at + 5)?, *b.get(at + 6)?]);
            return Some([u32::from(w), u32::from(h)]);
        }
        if len < 2 {
            return None;
        }
        at += len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) {
        assert!(
            (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn the_top_left_lands_on_the_origin_and_axes_follow_the_convention() {
        let p = Placement {
            origin: [3.0, -2.0],
            metres_per_pixel: 0.5,
            rotation_deg: 0.0,
        };
        close(p.pixel_to_world([0.0, 0.0]), [3.0, -2.0]);
        // image +x goes +X, image +y goes +Z
        close(p.pixel_to_world([10.0, 0.0]), [8.0, -2.0]);
        close(p.pixel_to_world([0.0, 10.0]), [3.0, 3.0]);
    }

    #[test]
    fn a_positive_rotation_turns_counter_clockwise_seen_from_above() {
        let p = Placement {
            origin: [0.0, 0.0],
            metres_per_pixel: 1.0,
            rotation_deg: 90.0,
        };
        // +x now points to -Z (up the drawing, north); +y points to +X
        close(p.pixel_to_world([1.0, 0.0]), [0.0, -1.0]);
        close(p.pixel_to_world([0.0, 1.0]), [1.0, 0.0]);
        // the same as Transform::rotation_y_deg: +90 takes +X to -Z
        let (s, c) = 90f32.to_radians().sin_cos();
        close([c * 1.0 + s * 0.0, -s * 1.0 + c * 0.0], [0.0, -1.0]);
    }

    #[test]
    fn pixel_and_world_round_trip() {
        let p = Placement {
            origin: [12.5, -7.25],
            metres_per_pixel: 0.037,
            rotation_deg: 33.0,
        };
        for pixel in [[0.0, 0.0], [640.0, 480.0], [-20.0, 1500.5]] {
            let back = p.world_to_pixel(p.pixel_to_world(pixel));
            assert!(
                (back[0] - pixel[0]).abs() < 0.01 && (back[1] - pixel[1]).abs() < 0.01,
                "{pixel:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn calibration_puts_both_pixels_on_their_points() {
        for (pa, pb, wa, wb) in [
            ([100.0, 200.0], [500.0, 200.0], [0.0, 0.0], [10.0, 0.0]),
            ([100.0, 200.0], [500.0, 200.0], [5.0, 5.0], [5.0, -5.0]),
            ([10.0, 10.0], [310.0, 410.0], [-4.0, 7.0], [-9.5, 1.25]),
            ([300.0, 90.0], [20.0, 600.0], [2.0, 2.0], [2.0, 12.0]),
        ] {
            let p = Placement::calibrated(pa, pb, wa, wb).unwrap();
            close(p.pixel_to_world(pa), wa);
            close(p.pixel_to_world(pb), wb);
        }
        // a 400 px wall that is 10 m long: 2.5 cm a pixel, no turn
        let p =
            Placement::calibrated([100.0, 200.0], [500.0, 200.0], [0.0, 0.0], [10.0, 0.0]).unwrap();
        assert!((p.metres_per_pixel - 0.025).abs() < 1e-6);
        assert!(p.rotation_deg.abs() < 1e-4);
        // a wall that must point north (-Z) turns the image by +90
        let p = Placement::calibrated([0.0, 0.0], [100.0, 0.0], [0.0, 0.0], [0.0, -10.0]).unwrap();
        assert!((p.rotation_deg - 90.0).abs() < 1e-3, "{}", p.rotation_deg);
    }

    #[test]
    fn calibration_refuses_equal_points() {
        assert!(Placement::calibrated([1.0, 1.0], [1.0, 1.0], [0.0, 0.0], [1.0, 0.0]).is_err());
        assert!(Placement::calibrated([0.0, 1.0], [1.0, 1.0], [2.0, 0.0], [2.0, 0.0]).is_err());
    }

    #[test]
    fn scaling_by_distance_keeps_origin_and_turn() {
        let p = Placement {
            origin: [1.0, 2.0],
            metres_per_pixel: 0.9,
            rotation_deg: 20.0,
        };
        let q = p
            .scaled_by_distance([0.0, 0.0], [300.0, 400.0], 12.5)
            .unwrap();
        assert_eq!((q.origin, q.rotation_deg), (p.origin, p.rotation_deg));
        assert!((q.metres_per_pixel - 0.025).abs() < 1e-6);
        assert!(p.scaled_by_distance([0.0, 0.0], [0.0, 0.0], 1.0).is_err());
        assert!(p.scaled_by_distance([0.0, 0.0], [1.0, 0.0], 0.0).is_err());
    }

    #[test]
    fn bounds_cover_a_rotated_image() {
        let u = Underlay {
            id: "u1".into(),
            name: "Plan".into(),
            file: "plan.png".into(),
            layer: "underlay".into(),
            placement: Placement {
                origin: [0.0, 0.0],
                metres_per_pixel: 0.1,
                rotation_deg: 90.0,
            },
            size_px: [200, 100],
            opacity: 0.5,
            elevation: 0.02,
            extensions: Default::default(),
        };
        // 20 x 10 m, turned: +x runs north, +y runs east
        let b = u.bounds();
        close(b[0], [0.0, -20.0]);
        close(b[1], [10.0, 0.0]);
    }

    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0]);
        b
    }

    #[test]
    fn sizes_are_read_from_png_and_jpeg_headers() {
        assert_eq!(image_size(&png_header(640, 480)), Ok([640, 480]));
        // SOI, an APP0 segment, then SOF0: precision, height 300, width 500
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46];
        jpeg.extend([
            0xFF, 0xC0, 0x00, 0x0B, 8, 0x01, 0x2C, 0x01, 0xF4, 1, 1, 0x11, 0,
        ]);
        assert_eq!(image_size(&jpeg), Ok([500, 300]));
    }

    #[test]
    fn other_bytes_and_cut_headers_are_refused_with_a_message() {
        assert!(image_size(b"").unwrap_err().contains("PNG or JPEG"));
        assert!(
            image_size(b"GIF89a....")
                .unwrap_err()
                .contains("PNG or JPEG")
        );
        let png = png_header(10, 10);
        for n in 0..24 {
            assert!(image_size(&png[..n]).is_err(), "{n}");
        }
        for n in 2..11 {
            let mut j = vec![
                0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 8, 0, 9, 0, 9, 1, 1, 0x11, 0,
            ];
            j.truncate(n);
            assert!(image_size(&j).is_err(), "{n}");
        }
        assert!(image_size(&png_header(0, 5)).is_err());
        assert!(
            image_size(&png_header(20000, 5))
                .unwrap_err()
                .contains("at most")
        );
    }
}
