//! Walking routes: a 2D plan of what a person cannot walk through (houses and blocks, walls and
//! fences, water, hedges, trunks and shrubs), a path planned round it with A* on an occupancy
//! grid, and the checks that say where a given path crosses something.

use crate::growth::{Grower, growth_of};
use crate::reference::Scene;
use crate::shape::Shape;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Half the width of the person (and the camera): how near an obstacle a route passes.
pub const BODY_RADIUS_M: f32 = 0.4;
/// The grid's cell, in metres (larger on a vast site).
const CELL_M: f32 = 0.25;
const MAX_CELLS: f32 = 600_000.0;
/// A hedge or shrub lower than this is stepped over; so is a wall.
const WALKABLE_HEIGHT_M: f32 = 0.5;

#[derive(Clone, Debug)]
enum Form {
    Circle { c: [f32; 2], r: f32 },
    Polygon(Vec<[f32; 2]>),
    Line { pts: Vec<[f32; 2]>, half: f32 },
}

#[derive(Clone, Debug)]
struct Obstacle {
    label: String,
    form: Form,
}

fn seg_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dz * dz;
    let t = if l2 < 1e-12 {
        0.0
    } else {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dz) / l2).clamp(0.0, 1.0)
    };
    (p[0] - (a[0] + dx * t)).hypot(p[1] - (a[1] + dz * t))
}

fn inside(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            c = !c;
        }
        j = i;
    }
    c
}

impl Obstacle {
    /// Signed distance from `p` to the obstacle's edge: negative inside.
    fn distance(&self, p: [f32; 2]) -> f32 {
        match &self.form {
            Form::Circle { c, r } => (p[0] - c[0]).hypot(p[1] - c[1]) - r,
            Form::Polygon(pts) => {
                let edge = (0..pts.len())
                    .map(|i| seg_distance(p, pts[i], pts[(i + 1) % pts.len()]))
                    .fold(f32::MAX, f32::min);
                if inside(p, pts) { -edge } else { edge }
            }
            Form::Line { pts, half } => {
                pts.windows(2)
                    .map(|w| seg_distance(p, w[0], w[1]))
                    .fold(f32::MAX, f32::min)
                    - half
            }
        }
    }

    fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let (pts, pad): (Vec<[f32; 2]>, f32) = match &self.form {
            Form::Circle { c, r } => (vec![*c], *r),
            Form::Polygon(p) => (p.clone(), 0.0),
            Form::Line { pts, half } => (pts.clone(), *half),
        };
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &pts {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        ([lo[0] - pad, lo[1] - pad], [hi[0] + pad, hi[1] + pad])
    }
}

/// What a walk cannot go through, as a plan.
#[derive(Clone, Debug, Default)]
pub struct Obstacles {
    list: Vec<Obstacle>,
}

/// Why a path is not clear: what it crosses, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct Crossing {
    pub what: String,
    /// Where the path first meets it.
    pub at: [f32; 2],
    /// Metres along the path.
    pub distance_m: f32,
}

impl Obstacles {
    /// Houses, blocks, walls, fences, water, and the hedges, shrubs and trunks of the plants
    /// `scene.years` after planting.
    pub fn from_scene(scene: &Scene) -> Obstacles {
        let p = scene.project;
        let mut list = Vec::new();
        let water = crate::plant_ground::water_areas(p);
        for s in &p.shapes {
            if !p.layer_visible(s.layer()) {
                continue;
            }
            let label = |kind: &str| format!("{kind} {} ({})", s.id(), s.name());
            match s {
                Shape::Extrusion(e) if e.stairs.is_none() && e.height >= WALKABLE_HEIGHT_M => {
                    let pts = e.outline().into_owned();
                    if e.closed && pts.len() >= 3 {
                        let kind = if e.house.is_some() { "house" } else { "block" };
                        list.push(Obstacle {
                            label: label(kind),
                            form: Form::Polygon(pts),
                        });
                    } else if !e.closed && pts.len() >= 2 {
                        let kind = if e.fence.is_some() { "fence" } else { "wall" };
                        list.push(Obstacle {
                            label: label(kind),
                            form: Form::Line {
                                pts,
                                half: (e.thickness / 2.0).max(0.04),
                            },
                        });
                    }
                }
                Shape::Area(a) if water.iter().any(|w| w == &a.id) => {
                    let pts = a.outline().into_owned();
                    if pts.len() >= 3 {
                        list.push(Obstacle {
                            label: label("water"),
                            form: Form::Polygon(pts),
                        });
                    }
                }
                _ => {}
            }
        }
        for g in &p.generators {
            if !p.layer_visible(g.layer()) {
                continue;
            }
            let hedge = format!("hedge or shrub {} ({})", g.id(), g.name());
            for (i, pl) in scene.placements_of(g).iter().enumerate() {
                let gr = growth_of(&pl.asset, None);
                let age = match scene.years {
                    Some(y) => g.planted_age_years().unwrap_or(gr.planted_age_years) + y,
                    None => gr.years_to_maturity * 4.0,
                };
                let size = gr.size(age);
                let (h, w) = (
                    gr.mature_height_m * size.height * pl.scale,
                    gr.mature_width_m * size.width * pl.scale,
                );
                let c = [pl.position[0], pl.position[2]];
                match gr.grower {
                    Grower::Tree | Grower::Conifer | Grower::Palm => {
                        let r = (gr.trunk_diameter_m(age) * pl.scale / 2.0).max(0.08);
                        list.push(Obstacle {
                            label: format!("trunk {}#{i} ({})", g.id(), g.name()),
                            form: Form::Circle { c, r },
                        });
                    }
                    Grower::Shrub | Grower::Clipped if h >= WALKABLE_HEIGHT_M => {
                        list.push(Obstacle {
                            label: hedge.clone(),
                            form: Form::Circle {
                                c,
                                r: (w / 2.0).max(0.12),
                            },
                        });
                    }
                    _ => {}
                }
            }
        }
        Obstacles { list }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The obstacles within `margin` of `p`, nearest first.
    fn near(&self, p: [f32; 2], margin: f32) -> Vec<(&str, f32)> {
        let mut v: Vec<(&str, f32)> = self
            .list
            .iter()
            .map(|o| (o.label.as_str(), o.distance(p)))
            .filter(|(_, d)| *d <= margin)
            .collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1));
        v
    }

    /// What `p` is in or within `margin` of, if anything.
    pub fn blocking(&self, p: [f32; 2], margin: f32) -> Option<String> {
        self.near(p, margin).first().map(|(l, _)| (*l).to_string())
    }

    /// The distance to the nearest obstacle's edge from `p` (0 inside), with its label.
    pub fn nearest(&self, p: [f32; 2]) -> Option<(String, f32)> {
        self.list
            .iter()
            .map(|o| (o.label.clone(), o.distance(p).max(0.0)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// What a path crosses (comes within `margin` of), once per thing, in the order met.
    pub fn crossings(&self, path: &[[f32; 2]], margin: f32) -> Vec<Crossing> {
        let mut out: Vec<Crossing> = Vec::new();
        let mut run = 0.0;
        for w in path.windows(2) {
            let len = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
            let n = (len / 0.1).ceil().max(1.0) as usize;
            for i in 0..=n {
                let t = i as f32 / n as f32;
                let p = [
                    w[0][0] + (w[1][0] - w[0][0]) * t,
                    w[0][1] + (w[1][1] - w[0][1]) * t,
                ];
                for (label, _) in self.near(p, margin) {
                    if !out.iter().any(|c| c.what == label) {
                        out.push(Crossing {
                            what: label.to_string(),
                            at: p,
                            distance_m: run + len * t,
                        });
                    }
                }
            }
            run += len;
        }
        out
    }
}

struct Grid {
    origin: [f32; 2],
    cell: f32,
    w: usize,
    h: usize,
    blocked: Vec<bool>,
}

impl Grid {
    fn build(obstacles: &Obstacles, lo: [f32; 2], hi: [f32; 2], clearance: f32) -> Grid {
        let size = [hi[0] - lo[0], hi[1] - lo[1]];
        let cell = CELL_M.max((size[0] * size[1] / MAX_CELLS).sqrt());
        let (w, h) = (
            (size[0] / cell).ceil().max(1.0) as usize,
            (size[1] / cell).ceil().max(1.0) as usize,
        );
        let mut grid = Grid {
            origin: lo,
            cell,
            w,
            h,
            blocked: vec![false; w * h],
        };
        for o in &obstacles.list {
            let (blo, bhi) = o.bounds();
            let c0 = grid.cell_of([blo[0] - clearance, blo[1] - clearance]);
            let c1 = grid.cell_of([bhi[0] + clearance, bhi[1] + clearance]);
            for y in c0.1..=c1.1 {
                for x in c0.0..=c1.0 {
                    if o.distance(grid.centre(x, y)) <= clearance {
                        grid.blocked[y * w + x] = true;
                    }
                }
            }
        }
        grid
    }

    fn cell_of(&self, p: [f32; 2]) -> (usize, usize) {
        let f =
            |v: f32, o: f32, n: usize| (((v - o) / self.cell).floor().max(0.0) as usize).min(n - 1);
        (
            f(p[0], self.origin[0], self.w),
            f(p[1], self.origin[1], self.h),
        )
    }

    fn centre(&self, x: usize, y: usize) -> [f32; 2] {
        [
            self.origin[0] + (x as f32 + 0.5) * self.cell,
            self.origin[1] + (y as f32 + 0.5) * self.cell,
        ]
    }

    fn is_blocked(&self, x: usize, y: usize) -> bool {
        self.blocked[y * self.w + x]
    }

    /// The free cell nearest to `from`, by rings.
    fn nearest_free(&self, from: (usize, usize)) -> Option<(usize, usize)> {
        let max = self.w.max(self.h);
        for r in 0..max as isize {
            let mut best: Option<((usize, usize), f32)> = None;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let (x, y) = (from.0 as isize + dx, from.1 as isize + dy);
                    if x < 0 || y < 0 || x >= self.w as isize || y >= self.h as isize {
                        continue;
                    }
                    let (x, y) = (x as usize, y as usize);
                    if !self.is_blocked(x, y) {
                        let d = ((dx * dx + dy * dy) as f32).sqrt();
                        if best.is_none_or(|b| d < b.1) {
                            best = Some(((x, y), d));
                        }
                    }
                }
            }
            if best.is_some() {
                return best.map(|b| b.0);
            }
        }
        None
    }

    fn line_clear(&self, a: (usize, usize), b: (usize, usize)) -> bool {
        let (mut x, mut y) = (a.0 as isize, a.1 as isize);
        let (x1, y1) = (b.0 as isize, b.1 as isize);
        let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
        let (sx, sy) = (if x < x1 { 1 } else { -1 }, if y < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        loop {
            if self.is_blocked(x as usize, y as usize) {
                return false;
            }
            if x == x1 && y == y1 {
                return true;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// A* over 8 neighbours (no corner cutting); the cells from `from` to `to`.
    fn astar(&self, from: (usize, usize), to: (usize, usize)) -> Option<Vec<(usize, usize)>> {
        #[derive(PartialEq)]
        struct Node(f32, usize);
        impl Eq for Node {}
        impl Ord for Node {
            fn cmp(&self, o: &Self) -> Ordering {
                o.0.total_cmp(&self.0)
            }
        }
        impl PartialOrd for Node {
            fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
                Some(self.cmp(o))
            }
        }
        let idx = |c: (usize, usize)| c.1 * self.w + c.0;
        let goal = idx(to);
        let octile = |i: usize| {
            let (x, y) = ((i % self.w) as f32, (i / self.w) as f32);
            let (dx, dy) = ((x - to.0 as f32).abs(), (y - to.1 as f32).abs());
            dx.max(dy) + (std::f32::consts::SQRT_2 - 1.0) * dx.min(dy)
        };
        let mut g = vec![f32::INFINITY; self.w * self.h];
        let mut came = vec![usize::MAX; self.w * self.h];
        let mut heap = BinaryHeap::new();
        g[idx(from)] = 0.0;
        heap.push(Node(octile(idx(from)), idx(from)));
        while let Some(Node(_, cur)) = heap.pop() {
            if cur == goal {
                let mut path = vec![(cur % self.w, cur / self.w)];
                let mut c = cur;
                while came[c] != usize::MAX {
                    c = came[c];
                    path.push((c % self.w, c / self.w));
                }
                path.reverse();
                return Some(path);
            }
            let (cx, cy) = ((cur % self.w) as isize, (cur / self.w) as isize);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (cx + dx, cy + dy);
                    if nx < 0 || ny < 0 || nx >= self.w as isize || ny >= self.h as isize {
                        continue;
                    }
                    let (nx, ny) = (nx as usize, ny as usize);
                    if self.is_blocked(nx, ny) {
                        continue;
                    }
                    if dx != 0
                        && dy != 0
                        && (self.is_blocked(nx, cy as usize) || self.is_blocked(cx as usize, ny))
                    {
                        continue;
                    }
                    let step = if dx != 0 && dy != 0 {
                        std::f32::consts::SQRT_2
                    } else {
                        1.0
                    };
                    let n = ny * self.w + nx;
                    let cost = g[cur] + step;
                    if cost < g[n] {
                        g[n] = cost;
                        came[n] = cur;
                        heap.push(Node(cost + octile(n), n));
                    }
                }
            }
        }
        None
    }
}

/// A planned route.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub path: Vec<[f32; 2]>,
    pub warnings: Vec<String>,
}

/// A path through `waypoints` that keeps `BODY_RADIUS_M` (or `clearance`) from everything in
/// `obstacles`. A waypoint inside an obstacle is moved to the nearest free spot, with a warning;
/// two waypoints with no clear way between them are joined by a straight line, with a warning.
pub fn plan(obstacles: &Obstacles, waypoints: &[[f32; 2]], clearance: Option<f32>) -> Route {
    let clearance = clearance.unwrap_or(BODY_RADIUS_M);
    let mut warnings = Vec::new();
    if waypoints.len() < 2 {
        return Route {
            path: waypoints.to_vec(),
            warnings,
        };
    }
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in waypoints {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let pad = 8.0 + 0.25 * (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let grid = Grid::build(
        obstacles,
        [lo[0] - pad, lo[1] - pad],
        [hi[0] + pad, hi[1] + pad],
        clearance,
    );
    // The waypoints on free ground.
    let mut spots: Vec<[f32; 2]> = Vec::new();
    for (i, w) in waypoints.iter().enumerate() {
        let cell = grid.cell_of(*w);
        if grid.is_blocked(cell.0, cell.1) {
            let what = obstacles
                .blocking(*w, clearance)
                .unwrap_or_else(|| "an obstacle".into());
            match grid.nearest_free(cell) {
                Some(free) => {
                    let c = grid.centre(free.0, free.1);
                    warnings.push(format!(
                        "waypoint {i} [{:.1}, {:.1}] is in or against {what}; the route goes to [{:.1}, {:.1}] instead",
                        w[0], w[1], c[0], c[1]
                    ));
                    spots.push(c);
                }
                None => {
                    warnings.push(format!(
                        "waypoint {i} [{:.1}, {:.1}] is in {what} and no free ground is near",
                        w[0], w[1]
                    ));
                    spots.push(*w);
                }
            }
        } else {
            spots.push(*w);
        }
    }
    let mut path = vec![spots[0]];
    for (i, pair) in spots.windows(2).enumerate() {
        let (a, b) = (grid.cell_of(pair[0]), grid.cell_of(pair[1]));
        let cells = if grid.is_blocked(a.0, a.1) || grid.is_blocked(b.0, b.1) {
            None
        } else {
            grid.astar(a, b)
        };
        match cells {
            Some(cells) => {
                // Pull the string: keep a cell only where the straight line on would be blocked.
                let mut kept = vec![cells[0]];
                let mut anchor = 0;
                for j in 2..cells.len() {
                    if !grid.line_clear(cells[anchor], cells[j]) {
                        kept.push(cells[j - 1]);
                        anchor = j - 1;
                    }
                }
                if cells.len() > 1 {
                    kept.push(*cells.last().expect("cells"));
                }
                for c in kept.iter().skip(1).take(kept.len().saturating_sub(2)) {
                    path.push(grid.centre(c.0, c.1));
                }
                path.push(pair[1]);
            }
            None => {
                warnings.push(format!(
                    "no clear path from waypoint {i} to {}: the way is closed (a hedge, wall or water all round); the route goes straight",
                    i + 1
                ));
                path.push(pair[1]);
            }
        }
    }
    path.dedup_by(|a, b| (a[0] - b[0]).hypot(a[1] - b[1]) < 0.02);
    Route { path, warnings }
}

/// The warnings for a path given by hand: what it crosses.
pub fn check_path(obstacles: &Obstacles, path: &[[f32; 2]]) -> Vec<String> {
    obstacles
        .crossings(path, 0.15)
        .into_iter()
        .map(|c| {
            format!(
                "the path crosses {} at [{:.1}, {:.1}] ({:.1} m along it); give `route` to have a way round planned",
                c.what, c.at[0], c.at[1], c.distance_m
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;

    fn garden() -> Project {
        crate::example("garden").unwrap()
    }

    fn length(p: &[[f32; 2]]) -> f32 {
        p.windows(2)
            .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
            .sum()
    }

    #[test]
    fn a_straight_path_through_the_hedge_is_a_warning_and_a_route_goes_round() {
        let p = garden();
        let scene = Scene::new(&p, Some(10.0));
        let obstacles = Obstacles::from_scene(&scene);
        assert!(!obstacles.is_empty());
        // The garden's hornbeam hedge g1 and the pond (water S6) are in the way.
        let hedge = obstacles
            .list
            .iter()
            .find(|o| o.label.contains("g1"))
            .expect("a hedge obstacle");
        let (lo, hi) = hedge.bounds();
        let across: Vec<[f32; 2]> = vec![
            [lo[0] - 3.0, (lo[1] + hi[1]) / 2.0],
            [hi[0] + 3.0, (lo[1] + hi[1]) / 2.0],
        ];
        let warnings = check_path(&obstacles, &across);
        assert!(
            warnings.iter().any(|w| w.contains("hedge or shrub g1")),
            "{warnings:?}"
        );
        // Through the pond.
        let through_pond = check_path(&obstacles, &[[5.0, 22.5], [16.0, 22.5]]);
        assert!(
            through_pond.iter().any(|w| w.contains("water S6")),
            "{through_pond:?}"
        );
        // A walk across the lawn is clear.
        assert!(check_path(&obstacles, &[[5.0, 14.0], [17.0, 14.0]]).is_empty());
        // A route round the pond keeps clear of it and is longer than the straight line.
        let route = plan(&obstacles, &[[5.0, 22.5], [16.0, 22.5]], None);
        assert!(route.warnings.is_empty(), "{:?}", route.warnings);
        assert!(length(&route.path) > 11.0 + 2.0, "{:?}", route.path);
        assert!(
            check_path(&obstacles, &route.path).is_empty(),
            "{:?} {:?}",
            route.path,
            check_path(&obstacles, &route.path)
        );
        for q in &route.path {
            assert!(obstacles.blocking(*q, 0.3).is_none(), "{q:?}");
        }
    }

    #[test]
    fn a_waypoint_in_the_pond_moves_to_its_edge_and_a_closed_way_warns() {
        let p = garden();
        let obstacles = Obstacles::from_scene(&Scene::new(&p, None));
        let route = plan(&obstacles, &[[5.0, 22.5], [10.5, 22.5]], None);
        assert!(
            route.warnings.iter().any(|w| w.contains("waypoint 1")),
            "{:?}",
            route.warnings
        );
        let end = route.path.last().unwrap();
        assert!(obstacles.blocking(*end, 0.1).is_none(), "{end:?}");
        // A hand-made box that closes a spot in.
        let mut boxed = Obstacles::default();
        boxed.list.push(Obstacle {
            label: "wall".into(),
            form: Form::Line {
                pts: vec![
                    [0.0, 0.0],
                    [10.0, 0.0],
                    [10.0, 10.0],
                    [0.0, 10.0],
                    [0.0, 0.0],
                ],
                half: 0.1,
            },
        });
        let closed = plan(&boxed, &[[5.0, 5.0], [15.0, 5.0]], None);
        assert!(
            closed.warnings.iter().any(|w| w.contains("no clear path")),
            "{:?}",
            closed.warnings
        );
        assert_eq!(closed.path.len(), 2);
    }

    #[test]
    fn crossings_are_listed_once_per_thing_with_where() {
        let mut o = Obstacles::default();
        o.list.push(Obstacle {
            label: "wall".into(),
            form: Form::Line {
                pts: vec![[5.0, -5.0], [5.0, 5.0]],
                half: 0.1,
            },
        });
        let c = o.crossings(&[[0.0, 0.0], [10.0, 0.0]], 0.15);
        assert_eq!(c.len(), 1);
        assert!((c[0].at[0] - 5.0).abs() < 0.3 && (c[0].distance_m - 5.0).abs() < 0.3);
        assert_eq!(o.nearest([2.0, 0.0]).unwrap().0, "wall");
    }
}
