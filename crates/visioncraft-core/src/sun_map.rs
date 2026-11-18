//! The sun-hours map: hours of direct sun over an area on a day, drawn on the ground
//! (`view.set_sun_map`). Headless: this module holds the setting stored with the project, the
//! grid over an outline, the colour ramp and the grid's texture; the host casts the rays
//! (`visioncraft_automation::inspect::SunGridJob`) and the renderer draws the texture.

use crate::project::Project;
use crate::season::Season;
use crate::shape::{Shape, inside_outline};
use crate::sun_hours::{FULL_SUN_HOURS, PART_SHADE_HOURS, days_in_month, season_date};
use serde::{Deserialize, Serialize};

/// The most cells a map samples (cell centres inside the outline).
pub const MAX_CELLS: usize = 10_000;
/// The hours of sun the colour ramp ends at: this many hours or more draw in its last colour.
pub const RAMP_HOURS: f32 = 8.0;
/// How far above the area the map floats (m): clear of the area's own z-fighting and of the
/// plan underlay.
pub const LIFT_M: f32 = 0.04;

/// A day, month and day of the project's year.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonthDay {
    pub month: u32,
    pub day: u32,
}

/// The map shown on the ground. Exactly one of `shape` and `outline` names the area; `date`
/// and `season` are alternatives (neither: the project's date).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SunMap {
    /// An area shape (`S3`); the map follows it when it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    /// Or a polygon `[x, z]` in metres (the whole plot is the box round the shapes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<Vec<[f32; 2]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<MonthDay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<Season>,
    /// Years after planting the crowns have grown; `None` follows the view's years.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub years: Option<f32>,
    #[serde(default = "default_cell_m")]
    pub cell_m: f32,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
}

fn default_cell_m() -> f32 {
    0.5
}

fn default_opacity() -> f32 {
    0.6
}

impl SunMap {
    /// Checks the setting against the project: one target, a usable date and numbers, and a
    /// grid of at most [`MAX_CELLS`] cells.
    pub fn validate(&self, project: &Project) -> Result<(), String> {
        if self.shape.is_some() == self.outline.is_some() {
            return Err("give exactly one of `shape` and `outline`".into());
        }
        if self.date.is_some() && self.season.is_some() {
            return Err("give `date` or `season`, not both".into());
        }
        if let Some(d) = self.date
            && (!(1..=12).contains(&d.month)
                || d.day < 1
                || d.day > days_in_month(project.time.year, d.month))
        {
            return Err(format!(
                "`date` {{month: {}, day: {}}} is not a day of {}",
                d.month, d.day, project.time.year
            ));
        }
        if let Some(y) = self.years
            && !(0.0..=crate::YEARS_MAX).contains(&y)
        {
            return Err(format!("`years` must be 0..{}", crate::YEARS_MAX));
        }
        if !(0.1..=10.0).contains(&self.cell_m) {
            return Err("`cell_m` must be 0.1..10".into());
        }
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err("`opacity` must be 0..1".into());
        }
        let outline = self.resolve_outline(project)?;
        let spec = GridSpec::new(&outline, self.cell_m);
        let n = spec.inside_count(&outline);
        if n == 0 {
            return Err("no cell centre lies inside the area; use a smaller `cell_m`".into());
        }
        if n > MAX_CELLS {
            return Err(format!(
                "the area has {n} cells at `cell_m` {}, at most {MAX_CELLS} are sampled; use a larger `cell_m` (about {:.1} m would do)",
                self.cell_m,
                self.cell_m * (n as f32 / MAX_CELLS as f32).sqrt() * 1.1
            ));
        }
        Ok(())
    }

    /// The polygon the map covers: the area's outline (curves cut into segments) or the given
    /// one.
    pub fn resolve_outline(&self, project: &Project) -> Result<Vec<[f32; 2]>, String> {
        let outline = match (&self.shape, &self.outline) {
            (Some(id), _) => match project.shapes.iter().find(|s| s.id() == id) {
                Some(Shape::Area(a)) => a.outline().into_owned(),
                Some(Shape::Extrusion(_)) => {
                    return Err(format!("`{id}` is not an area: give an area's id"));
                }
                None => return Err(format!("no shape `{id}`")),
            },
            (None, Some(o)) => o.clone(),
            (None, None) => return Err("give `shape` or `outline`".into()),
        };
        if outline.len() < 3 {
            return Err("an outline needs at least 3 points".into());
        }
        if outline.iter().flatten().any(|v| !v.is_finite()) {
            return Err("an outline point is not a usable number".into());
        }
        Ok(outline)
    }

    /// The day studied: the given date, the season's day at the site, or the project's date.
    pub fn day(&self, project: &Project) -> (u32, u32) {
        match (self.date, self.season) {
            (Some(d), _) => (d.month, d.day),
            (None, Some(s)) => season_date(s, project.site.latitude),
            (None, None) => (project.time.month, project.time.day),
        }
    }

    /// Years after planting the crowns have grown.
    pub fn years_for(&self, project: &Project) -> f32 {
        self.years.or(project.view.years).unwrap_or(0.0)
    }

    /// The height (m) the map floats at over the target area: the area's elevation, else the
    /// lowest height of the ground plane, plus [`LIFT_M`].
    pub fn elevation(&self, project: &Project) -> f32 {
        let area = self.shape.as_ref().and_then(|id| {
            project.shapes.iter().find_map(|s| match s {
                Shape::Area(a) if a.id == *id => Some(a.elevation),
                _ => None,
            })
        });
        let base = area.unwrap_or_else(|| project.view.ground.map_or(0.0, |g| g.height));
        base + LIFT_M
    }
}

/// The cells over an outline: `cols` x `rows` squares of `cell_m` from `origin` (the lowest x
/// and z), row 0 at the lowest z, the north side, columns running east.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSpec {
    pub origin: [f32; 2],
    pub cols: usize,
    pub rows: usize,
    pub cell_m: f32,
}

impl GridSpec {
    pub fn new(outline: &[[f32; 2]], cell_m: f32) -> Self {
        let lo = [0, 1].map(|k| outline.iter().map(|v| v[k]).fold(f32::INFINITY, f32::min));
        let hi = [0, 1].map(|k| {
            outline
                .iter()
                .map(|v| v[k])
                .fold(f32::NEG_INFINITY, f32::max)
        });
        let size = |k: usize| (((hi[k] - lo[k]) / cell_m).ceil() as usize).max(1);
        Self {
            origin: lo,
            cols: size(0),
            rows: size(1),
            cell_m,
        }
    }

    /// The centre `[x, z]` of cell `i` (row-major).
    pub fn centre(&self, i: usize) -> [f32; 2] {
        [
            self.origin[0] + ((i % self.cols) as f32 + 0.5) * self.cell_m,
            self.origin[1] + ((i / self.cols) as f32 + 0.5) * self.cell_m,
        ]
    }

    /// The centre of every cell that lies inside `outline`, `None` outside.
    pub fn cells(&self, outline: &[[f32; 2]]) -> Vec<Option<[f32; 2]>> {
        (0..self.cols * self.rows)
            .map(|i| {
                let c = self.centre(i);
                inside_outline(c, outline).then_some(c)
            })
            .collect()
    }

    /// How many cell centres lie inside `outline`; stops counting well past [`MAX_CELLS`].
    pub fn inside_count(&self, outline: &[[f32; 2]]) -> usize {
        if self.cols.saturating_mul(self.rows) > 25 * MAX_CELLS {
            return self.cols * self.rows;
        }
        (0..self.cols * self.rows)
            .filter(|i| inside_outline(self.centre(*i), outline))
            .count()
    }

    /// The grid's corners in the ground plane `[x, z]`: north-west, north-east, south-east,
    /// south-west (the corners of a picture whose row 0 is the north side).
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let [x0, z0] = self.origin;
        let (x1, z1) = (
            x0 + self.cols as f32 * self.cell_m,
            z0 + self.rows as f32 * self.cell_m,
        );
        [[x0, z0], [x1, z0], [x1, z1], [x0, z1]]
    }
}

/// The ramp's stops: `(t, sRGB)` from deep blue-violet through blue, teal and green to yellow
/// and amber. Lightness grows along it (checked by a test) so it reads in grey and for
/// colour-blind eyes, in the manner of viridis and cividis.
const STOPS: [(f32, [u8; 3]); 8] = [
    (0.0, [44, 18, 100]),
    (0.17, [52, 52, 160]),
    (0.34, [30, 110, 170]),
    (0.5, [20, 150, 150]),
    (0.66, [60, 175, 95]),
    (0.8, [165, 198, 48]),
    (0.9, [222, 206, 58]),
    (1.0, [255, 205, 85]),
];

/// The colour (sRGB) of `hours` of sun: 0 h deep violet, [`RAMP_HOURS`] and more amber.
pub fn ramp(hours: f32) -> [u8; 3] {
    let t = (hours / RAMP_HOURS).clamp(0.0, 1.0);
    let t = if t.is_nan() { 0.0 } else { t };
    let i = STOPS
        .iter()
        .position(|(s, _)| *s >= t)
        .unwrap_or(STOPS.len() - 1)
        .max(1);
    let ((t0, a), (t1, b)) = (STOPS[i - 1], STOPS[i]);
    let f = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
    [0, 1, 2].map(|k| (f32::from(a[k]) + (f32::from(b[k]) - f32::from(a[k])) * f).round() as u8)
}

/// The grid as an RGBA8 picture, one texel per cell: texel (col, row) is the cell at that column
/// and row, so row 0 is the north side (lowest z) and the picture's top edge lies on the grid's
/// north edge (`GridSpec::corners`: top-left is the first corner). Cells outside the outline
/// (`None`) are transparent; their colour is the mean of the sunny neighbours so that smooth
/// sampling at the edge does not darken it.
pub fn texture_rgba(spec: &GridSpec, hours: &[Option<f64>]) -> Vec<u8> {
    let (cols, rows) = (spec.cols, spec.rows);
    let colour = |c: isize, r: isize| -> Option<[u8; 3]> {
        if c < 0 || r < 0 || c >= cols as isize || r >= rows as isize {
            return None;
        }
        hours
            .get(r as usize * cols + c as usize)
            .copied()
            .flatten()
            .map(|h| ramp(h as f32))
    };
    let mut out = Vec::with_capacity(cols * rows * 4);
    for r in 0..rows as isize {
        for c in 0..cols as isize {
            if let Some(rgb) = colour(c, r) {
                out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                continue;
            }
            let near: Vec<[u8; 3]> = (-1..=1)
                .flat_map(|dr| (-1..=1).map(move |dc| (dc, dr)))
                .filter_map(|(dc, dr)| colour(c + dc, r + dr))
                .collect();
            let mean = |k: usize| {
                if near.is_empty() {
                    0
                } else {
                    (near.iter().map(|v| u32::from(v[k])).sum::<u32>() / near.len() as u32) as u8
                }
            };
            out.extend_from_slice(&[mean(0), mean(1), mean(2), 0]);
        }
    }
    out
}

/// What a finished map says about itself, for the legend.
#[derive(Clone, Debug, PartialEq)]
pub struct SunMapStats {
    pub month: u32,
    pub day: u32,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub daylight_hours: f64,
    /// Shares of the cells (0..=1): [`FULL_SUN_HOURS`] and more, between it and
    /// [`PART_SHADE_HOURS`], under.
    pub full_sun_share: f64,
    pub part_shade_share: f64,
    pub shade_share: f64,
}

impl SunMapStats {
    /// `None` when no cell is inside.
    pub fn of(month: u32, day: u32, daylight_hours: f64, hours: &[Option<f64>]) -> Option<Self> {
        let inside: Vec<f64> = hours.iter().flatten().copied().collect();
        if inside.is_empty() {
            return None;
        }
        let n = inside.len() as f64;
        let share = |f: &dyn Fn(f64) -> bool| inside.iter().filter(|h| f(**h)).count() as f64 / n;
        Some(Self {
            month,
            day,
            min: inside.iter().copied().fold(f64::INFINITY, f64::min),
            max: inside.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            mean: inside.iter().sum::<f64>() / n,
            daylight_hours,
            full_sun_share: share(&|h| h >= FULL_SUN_HOURS),
            part_shade_share: share(&|h| (PART_SHADE_HOURS..FULL_SUN_HOURS).contains(&h)),
            shade_share: share(&|h| h < PART_SHADE_HOURS),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lightness(c: [u8; 3]) -> f32 {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let y = 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
        // CIE lightness L*.
        if y > 0.008856 {
            116.0 * y.cbrt() - 16.0
        } else {
            903.3 * y
        }
    }

    #[test]
    fn the_ramp_runs_from_deep_violet_to_amber_and_gets_lighter() {
        let (lo, hi) = (ramp(0.0), ramp(RAMP_HOURS));
        assert_eq!(lo, STOPS[0].1);
        assert_eq!(hi, STOPS[7].1);
        // Blue-violet at the dark end, warm at the light end.
        assert!(lo[2] > lo[0] && lo[2] > lo[1], "{lo:?}");
        assert!(hi[0] > hi[1] && hi[1] > hi[2], "{hi:?}");
        assert_eq!(ramp(20.0), hi);
        assert_eq!(ramp(-1.0), lo);
        let mut last = -1.0;
        for i in 0..=160 {
            let y = lightness(ramp(i as f32 * 0.05));
            assert!(y >= last - 0.3, "lightness drops at {} h", i as f32 * 0.05);
            last = y;
        }
        // Teal and green sit on the way.
        let mid = ramp(4.0);
        assert!(mid[1] > mid[0] && mid[2] > mid[0], "{mid:?}");
    }

    fn l_shape() -> Vec<[f32; 2]> {
        // An L: the lower-z (north) bar is 4 m wide, the south leg only 2 m wide.
        vec![
            [10.0, 20.0],
            [14.0, 20.0],
            [14.0, 22.0],
            [12.0, 22.0],
            [12.0, 26.0],
            [10.0, 26.0],
        ]
    }

    #[test]
    fn the_grid_texture_is_transparent_outside_and_not_flipped() {
        let outline = l_shape();
        let spec = GridSpec::new(&outline, 1.0);
        assert_eq!((spec.cols, spec.rows), (4, 6));
        assert_eq!(spec.origin, [10.0, 20.0]);
        let cells = spec.cells(&outline);
        // Hours encode the cell's world position: x grows eastwards, z southwards.
        let hours: Vec<Option<f64>> = cells
            .iter()
            .map(|c| c.map(|[x, z]| f64::from(x - 10.0) + f64::from(z - 20.0) * 0.1))
            .collect();
        let tex = texture_rgba(&spec, &hours);
        assert_eq!(tex.len(), 4 * 6 * 4);
        let texel = |col: usize, row: usize| &tex[(row * 4 + col) * 4..][..4];
        for row in 0..6 {
            for col in 0..4 {
                // Where the texel lies in the world (the picture's top-left on corner 0).
                let c = spec.corners();
                let x = c[0][0] + (col as f32 + 0.5) / 4.0 * (c[1][0] - c[0][0]);
                let z = c[0][1] + (row as f32 + 0.5) / 6.0 * (c[3][1] - c[0][1]);
                let inside = inside_outline([x, z], &outline);
                assert_eq!(texel(col, row)[3] == 255, inside, "({col}, {row})");
                if inside {
                    let h = f64::from(x - 10.0) + f64::from(z - 20.0) * 0.1;
                    assert_eq!(texel(col, row)[..3], ramp(h as f32), "({col}, {row})");
                }
            }
        }
        // The cut-out corner (south-east) is clear, the north row is full, the north-west
        // corner is the first row's first texel.
        assert_eq!(texel(3, 5)[3], 0);
        assert_eq!(texel(3, 0)[3], 255);
        assert_eq!(texel(0, 5)[3], 255);
        assert_eq!(texel(0, 0)[..3], ramp(0.55));
        assert_eq!(texel(3, 0)[..3], ramp(3.55));
        // Transparent texels take their neighbours' colour, not black.
        assert_eq!(texel(3, 2)[3], 0);
        assert_ne!(texel(3, 2)[..3], [0, 0, 0][..]);
    }

    #[test]
    fn a_setting_is_checked_against_the_project() {
        use serde_json::json;
        let mut s = crate::Session::new(Project::default());
        s.run(
            "shape.add_area",
            json!({"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]]}),
        )
        .unwrap();
        let p = s.project();
        let map: SunMap = serde_json::from_value(json!({"shape": "S1"})).unwrap();
        assert_eq!((map.cell_m, map.opacity), (0.5, 0.6));
        map.validate(p).unwrap();
        assert_eq!(map.resolve_outline(p).unwrap().len(), 4);
        assert!((map.elevation(p) - (0.0 + LIFT_M)).abs() < 0.2);
        for bad in [
            json!({}),
            json!({"shape": "S1", "outline": [[0, 0], [1, 0], [1, 1]]}),
            json!({"shape": "S9"}),
            json!({"shape": "S1", "season": "winter", "date": {"month": 1, "day": 1}}),
            json!({"shape": "S1", "date": {"month": 2, "day": 30}}),
            json!({"shape": "S1", "cell_m": 0.01}),
            json!({"shape": "S1", "opacity": 2}),
            json!({"shape": "S1", "years": 99}),
            json!({"outline": [[0, 0], [1, 0]]}),
            json!({"outline": [[0, 0], [1000, 0], [1000, 1000]], "cell_m": 0.1}),
        ] {
            let map: SunMap = serde_json::from_value(bad.clone()).unwrap();
            assert!(map.validate(p).is_err(), "{bad}");
        }
        // The day: a season is its equinox or solstice, else the project's date.
        let w: SunMap = serde_json::from_value(json!({"shape": "S1", "season": "winter"})).unwrap();
        assert_eq!(w.day(p), (12, 21));
        assert_eq!(map_day_default(p), (p.time.month, p.time.day));
    }

    fn map_day_default(p: &Project) -> (u32, u32) {
        let m: SunMap = serde_json::from_value(serde_json::json!({"shape": "S1"})).unwrap();
        m.day(p)
    }

    #[test]
    fn stats_count_the_shares_of_the_cells_inside() {
        let hours = [Some(8.0), Some(4.0), Some(1.0), None, Some(6.0)];
        let s = SunMapStats::of(12, 21, 8.0, &hours).unwrap();
        assert_eq!((s.min, s.max, s.mean), (1.0, 8.0, 4.75));
        assert_eq!(
            (s.full_sun_share, s.part_shade_share, s.shade_share),
            (0.5, 0.25, 0.25)
        );
        assert!(SunMapStats::of(1, 1, 8.0, &[None]).is_none());
    }
}
