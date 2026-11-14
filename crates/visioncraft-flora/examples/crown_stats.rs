//! Numbers behind how a broadleaf crown reads, per species, age and level of detail: triangles,
//! foliage cards and their size, how large a leaf is drawn against its real size, about how many
//! leaves one card shows, how much of the crown's side and top silhouette the cards cover (alpha
//! tested against the drawn atlas), and how large a leaf is on screen at a few distances.
//!
//! Levels are those of `generate_levels` with the leaf-level detail: 0 every shoot's leaves,
//! 1 sprays at the larger budget, 2 to 4 the three card levels. `ms` is the time for all levels
//! of one tree (skeleton once), and `winter` the near level's triangles out of leaf.
//!
//! `cargo run --release -p visioncraft-flora --example crown_stats -- [species ids...]`

use visioncraft_flora::{
    ATLAS_CELLS, ATLAS_SIZE, Image, Material, PlantMesh, Season, Species, generate_levels, textures,
};

/// Years to maturity and planting age as in `visioncraft-core`'s growth table (estimated there).
const TREES: &[(&str, f32)] = &[
    ("quercus-robur", 60.0),
    ("juglans-regia", 40.0),
    ("acer-campestre", 30.0),
    ("betula-pendula", 18.0),
    ("platanus-hispanica", 40.0),
    ("malus-domestica", 15.0),
];

/// The growth curve of `visioncraft-core::growth` for trees, rounded to tenths as the app does.
fn age_fraction(years: f32, maturity: f32) -> f32 {
    let p = 1.6f32;
    let k = -(1.0 - 0.95f32.powf(1.0 / p)).ln() / maturity;
    let s = (1.0 - (-k * years).exp()).powf(p).min(1.0);
    (s * 10.0).round().clamp(1.0, 10.0) / 10.0
}

/// Alpha of the atlas at `uv` (nearest texel).
fn alpha(a: &Image, uv: [f32; 2]) -> u8 {
    let x = ((uv[0] * a.width as f32) as u32).min(a.width - 1);
    let y = ((uv[1] * a.height as f32) as u32).min(a.height - 1);
    a.rgba[((y * a.width + x) * 4 + 3) as usize]
}

/// Fraction of the silhouette ellipse (axes `u`, `v` of the positions) covered by opaque foliage
/// texels, on an `n` x `n` grid, plus the mean number of opaque layers there.
fn coverage(m: &PlantMesh, atlas: &Image, u: usize, v: usize) -> (f32, f32) {
    const N: usize = 160;
    let foliage: Vec<u32> = m
        .parts
        .iter()
        .filter(|p| p.material == Material::Foliage)
        .flat_map(|p| m.indices[p.index_range.start as usize..p.index_range.end as usize].to_vec())
        .collect();
    if foliage.is_empty() {
        return (0.0, 0.0);
    }
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for &i in &foliage {
        let p = m.positions[i as usize];
        lo = [lo[0].min(p[u]), lo[1].min(p[v])];
        hi = [hi[0].max(p[u]), hi[1].max(p[v])];
    }
    let span = [(hi[0] - lo[0]).max(1e-3), (hi[1] - lo[1]).max(1e-3)];
    let mut layers = vec![0u16; N * N];
    for t in foliage.chunks(3) {
        let q: Vec<([f32; 2], [f32; 2])> = t
            .iter()
            .map(|&i| {
                let p = m.positions[i as usize];
                (
                    [
                        (p[u] - lo[0]) / span[0] * N as f32,
                        (p[v] - lo[1]) / span[1] * N as f32,
                    ],
                    m.uvs[i as usize],
                )
            })
            .collect();
        let (a, b, c) = (q[0].0, q[1].0, q[2].0);
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        if area.abs() < 1e-6 {
            continue;
        }
        let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let x1 = (a[0].max(b[0]).max(c[0]).ceil() as usize).min(N);
        let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
        let y1 = (a[1].max(b[1]).max(c[1]).ceil() as usize).min(N);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let w1 = ((p[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (p[1] - a[1])) / area;
                let w2 = ((b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1])) / area;
                let w0 = 1.0 - w1 - w2;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let uv = [
                    q[0].1[0] * w0 + q[1].1[0] * w1 + q[2].1[0] * w2,
                    q[0].1[1] * w0 + q[1].1[1] * w1 + q[2].1[1] * w2,
                ];
                if alpha(atlas, uv) >= 128 {
                    layers[y * N + x] += 1;
                }
            }
        }
    }
    let (mut inside, mut covered, mut depth) = (0, 0, 0u32);
    for y in 0..N {
        for x in 0..N {
            let dx = (x as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            let dy = (y as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            if dx * dx + dy * dy <= 0.8 {
                inside += 1;
                let l = layers[y * N + x];
                if l > 0 {
                    covered += 1;
                    depth += l as u32;
                }
            }
        }
    }
    (
        covered as f32 / inside as f32,
        depth as f32 / covered.max(1) as f32,
    )
}

/// Opaque fraction of each atlas cell (16 cells).
fn cell_fill(atlas: &Image) -> Vec<f32> {
    let c = (ATLAS_SIZE / ATLAS_CELLS) as usize * atlas.width as usize / ATLAS_SIZE as usize;
    (0..16)
        .map(|cell| {
            let (cx, cy) = (cell % 4 * c, cell / 4 * c);
            let mut n = 0;
            for y in cy..cy + c {
                for x in cx..cx + c {
                    if atlas.rgba[(y * atlas.width as usize + x) * 4 + 3] >= 128 {
                        n += 1;
                    }
                }
            }
            n as f32 / (c * c) as f32
        })
        .collect()
}

fn main() {
    let ids: Vec<String> = std::env::args().skip(1).collect();
    println!(
        "species age_y age lod tris wood cards card_m cell leaf_x leaves/card cov_side layers cov_top px@6 px@15 px@40 ms winter"
    );
    for &(id, maturity) in TREES {
        if !ids.is_empty() && !ids.iter().any(|i| i == id) {
            continue;
        }
        let sp = Species::by_id(id).expect("species");
        let atlas = textures(&sp.params).atlas;
        let fill = cell_fill(&atlas);
        for years in [10.0, 20.0, 40.0] {
            let mut p = sp.params.clone();
            p.age = age_fraction(years, maturity);
            let t0 = std::time::Instant::now();
            let levels = generate_levels(&p, 7, Some(Season { month: 7 }), true);
            let ms = t0.elapsed().as_secs_f32() * 1000.0;
            let winter =
                generate_levels(&p, 7, Some(Season { month: 1 }), true)[0].triangle_count();
            for (li, m) in levels.iter().enumerate() {
                let cards = m.triangles_of(Material::Foliage) / 2;
                // The first foliage card: its side, and which atlas row its cell is in.
                let first = m.parts.iter().find(|q| q.material == Material::Foliage);
                let (size, row, cell) = first.map_or((0.0, 0, 0), |q| {
                    let i = &m.indices[q.index_range.start as usize..];
                    let (a, b) = (m.positions[i[0] as usize], m.positions[i[1] as usize]);
                    let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2))
                        .sqrt();
                    let uv = m.uvs[i[0] as usize];
                    let cell = (uv[1] * 4.0) as usize * 4 + (uv[0] * 4.0) as usize;
                    (d, cell / 4, cell)
                });
                let lf = &p.leaves;
                // A spray cell holds leaves at true size for a card of `card_m`; a clump cell
                // stands for a card 2.0 times that (texture::CLUMP_SCALE).
                let leaf_x = if row == 1 {
                    size / (2.0 * lf.card_m)
                } else if row == 3 {
                    size / (2.6 * lf.leaf_m)
                } else {
                    size / lf.card_m
                };
                let leaf_area = (lf.leaf_m * leaf_x).powi(2) * lf.aspect.clamp(0.2, 1.0) * 0.65;
                let leaves = fill[cell] * size * size / leaf_area.max(1e-6);
                let (cs, depth) = coverage(m, &atlas, 0, 1);
                let (ct, _) = coverage(m, &atlas, 0, 2);
                // On a 1080 px tall view with a 50 degree field of view.
                let px = |d: f32| lf.leaf_m * leaf_x / (2.0 * d * 0.4663) * 1080.0;
                println!(
                    "{id} {years} {:.1} {} {} {} {cards} {size:.2} {} {leaf_x:.2} {leaves:.0} {cs:.2} {depth:.1} {ct:.2} {:.1} {:.1} {:.1} {ms:.0} {winter}",
                    p.age,
                    li,
                    m.triangle_count(),
                    m.triangles_of(Material::Bark),
                    if row == 1 { "clump" } else { "spray" },
                    px(6.0),
                    px(15.0),
                    px(40.0),
                );
            }
        }
    }
}
