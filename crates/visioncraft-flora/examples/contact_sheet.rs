//! Software preview of every species: orthographic side and top views at each LOD, with
//! alpha-cut texturing, sun-plus-ambient shading, a depth buffer and a 1.8 m human for scale,
//! tiled into contact sheets. Also dumps each leaf atlas and bark texture and prints triangle
//! counts and generation times.
//!
//! `cargo run --release -p visioncraft-flora --example contact_sheet -- [out_dir]`

use std::time::Instant;

use visioncraft_flora::{
    Category, Image, Lod, Material, PlantMesh, PlantTextures, Season, generate, generate_in_season,
    species, textures,
};

const PANEL: usize = 256;
const SS: usize = 2;
const ROWS_PER_SHEET: usize = 4;
const LABEL_H: usize = 14;

struct Canvas {
    w: usize,
    h: usize,
    rgb: Vec<[f32; 3]>,
}

impl Canvas {
    fn new(w: usize, h: usize, c: [f32; 3]) -> Canvas {
        Canvas {
            w,
            h,
            rgb: vec![c; w * h],
        }
    }
    fn set(&mut self, x: i64, y: i64, c: [f32; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.rgb[y as usize * self.w + x as usize] = c;
        }
    }
    fn blit(&mut self, src: &Canvas, ox: usize, oy: usize) {
        for y in 0..src.h {
            for x in 0..src.w {
                self.set((ox + x) as i64, (oy + y) as i64, src.rgb[y * src.w + x]);
            }
        }
    }
    fn save(&self, path: &str) {
        let data: Vec<u8> = self
            .rgb
            .iter()
            .flat_map(|c| c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
            .collect();
        write_png(
            path,
            self.w as u32,
            self.h as u32,
            png::ColorType::Rgb,
            &data,
        );
    }
}

fn write_png(path: &str, w: u32, h: u32, color: png::ColorType, data: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(color);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(data)
        .expect("png data");
}

fn sample(img: &Image, u: f32, v: f32) -> [f32; 4] {
    let x = ((u * img.width as f32) as i64).clamp(0, img.width as i64 - 1) as usize;
    let y = ((v * img.height as f32) as i64).clamp(0, img.height as i64 - 1) as usize;
    let i = (y * img.width as usize + x) * 4;
    let p = &img.rgba[i..i + 4];
    [
        p[0] as f32 / 255.0,
        p[1] as f32 / 255.0,
        p[2] as f32 / 255.0,
        p[3] as f32 / 255.0,
    ]
}

#[derive(Clone, Copy)]
enum View {
    Side,
    Top,
}

/// World extents shared by a species' panels: (x0, x1, y0, y1) in view-plane coordinates.
type Frame = (f32, f32, f32, f32);

fn project(view: View, p: [f32; 3]) -> (f32, f32, f32) {
    match view {
        View::Side => (p[0], p[1], p[2]), // screen x, screen y (up), depth (towards viewer)
        View::Top => (p[0], -p[2], p[1]), // looking down, +Z at the bottom of the image
    }
}

const HUMAN_X_GAP: f32 = 0.5;

fn frame_for(mesh: &PlantMesh, view: View) -> Frame {
    let (lo, hi) = mesh.bounds();
    let hx = lo[0] - HUMAN_X_GAP - 0.25;
    match view {
        View::Side => {
            let (x0, x1) = (hx - 0.3, hi[0]);
            let (y0, y1) = (-0.05, hi[1].max(1.9));
            square((x0, x1, y0, y1))
        }
        View::Top => square((hx - 0.3, hi[0], -hi[2], -lo[2])),
    }
}

fn square(f: Frame) -> Frame {
    let (w, h) = (f.1 - f.0, f.3 - f.2);
    let s = w.max(h) * 1.06;
    let (cx, cy) = ((f.0 + f.1) * 0.5, (f.2 + f.3) * 0.5);
    (cx - s * 0.5, cx + s * 0.5, cy - s * 0.5, cy + s * 0.5)
}

fn render(
    mesh: &PlantMesh,
    tex: &PlantTextures,
    view: View,
    frame: Frame,
    label_lod: usize,
) -> Canvas {
    let n = PANEL * SS;
    let sky = match view {
        View::Side => [0.86, 0.89, 0.92],
        View::Top => [0.78, 0.8, 0.74],
    };
    let mut col = vec![sky; n * n];
    let mut depth = vec![f32::MIN; n * n];
    let to_px = |x: f32, y: f32| -> (f32, f32) {
        (
            (x - frame.0) / (frame.1 - frame.0) * n as f32,
            (frame.3 - y) / (frame.3 - frame.2) * n as f32,
        )
    };
    // Ground line and human silhouette (behind the plant).
    let (lo, _) = mesh.bounds();
    let hx = lo[0] - HUMAN_X_GAP;
    let human = [0.25, 0.25, 0.3];
    for py in 0..n {
        for px in 0..n {
            let wx = frame.0 + (px as f32 + 0.5) / n as f32 * (frame.1 - frame.0);
            let wy = frame.3 - (py as f32 + 0.5) / n as f32 * (frame.3 - frame.2);
            let inside = match view {
                View::Side => {
                    if wy < 0.0 {
                        col[py * n + px] = [0.55, 0.5, 0.42];
                    }
                    let body = (wx - hx).abs() < 0.2
                        && wy > 0.0
                        && wy < 1.45
                        && !((wx - hx).abs() < 0.03 && wy < 0.8);
                    let head = (wx - hx) * (wx - hx) + (wy - 1.65) * (wy - 1.65) < 0.11 * 0.11;
                    body || head
                }
                View::Top => (wx - hx) * (wx - hx) + wy * wy < 0.22 * 0.22,
            };
            if inside {
                col[py * n + px] = human;
            }
        }
    }
    let sun = norm([0.45, 0.8, 0.4]);
    let tint = mesh.foliage_tint;
    for part in &mesh.parts {
        let img = if part.material == Material::Bark {
            &tex.bark
        } else {
            &tex.atlas
        };
        let r = part.index_range.start as usize..part.index_range.end as usize;
        for t in mesh.indices[r].chunks(3) {
            let v = [t[0] as usize, t[1] as usize, t[2] as usize];
            let pr: Vec<(f32, f32, f32)> = v
                .iter()
                .map(|&i| project(view, mesh.positions[i]))
                .collect();
            let sp: Vec<(f32, f32)> = pr.iter().map(|q| to_px(q.0, q.1)).collect();
            let area = (sp[1].0 - sp[0].0) * (sp[2].1 - sp[0].1)
                - (sp[2].0 - sp[0].0) * (sp[1].1 - sp[0].1);
            if area.abs() < 1e-9 {
                continue;
            }
            let x0 = sp
                .iter()
                .map(|q| q.0)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize;
            let x1 = (sp.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil() as usize).min(n);
            let y0 = sp
                .iter()
                .map(|q| q.1)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0) as usize;
            let y1 = (sp.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as usize).min(n);
            for py in y0..y1 {
                for px in x0..x1 {
                    let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                    let w0 =
                        ((sp[1].0 - fx) * (sp[2].1 - fy) - (sp[2].0 - fx) * (sp[1].1 - fy)) / area;
                    let w1 =
                        ((sp[2].0 - fx) * (sp[0].1 - fy) - (sp[0].0 - fx) * (sp[2].1 - fy)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * pr[0].2 + w1 * pr[1].2 + w2 * pr[2].2;
                    let idx = py * n + px;
                    if z <= depth[idx] {
                        continue;
                    }
                    let uv = lerp3(mesh.uvs[v[0]], mesh.uvs[v[1]], mesh.uvs[v[2]], w0, w1, w2);
                    let c = sample(img, uv[0], uv[1]);
                    if part.material != Material::Bark && c[3] < 0.5 {
                        continue;
                    }
                    let nn = norm(lerp3(
                        mesh.normals[v[0]],
                        mesh.normals[v[1]],
                        mesh.normals[v[2]],
                        w0,
                        w1,
                        w2,
                    ));
                    let lambert = (nn[0] * sun[0] + nn[1] * sun[1] + nn[2] * sun[2]).max(0.0);
                    let light = 0.42 + 0.7 * lambert;
                    let k = if part.material == Material::Foliage {
                        tint
                    } else {
                        [1.0; 3]
                    };
                    depth[idx] = z;
                    col[idx] = [
                        c[0] * k[0] * light,
                        c[1] * k[1] * light,
                        c[2] * k[2] * light,
                    ];
                }
            }
        }
    }
    // Downsample.
    let mut out = Canvas::new(PANEL, PANEL, sky);
    for y in 0..PANEL {
        for x in 0..PANEL {
            let mut acc = [0.0; 3];
            for sy in 0..SS {
                for sx in 0..SS {
                    let c = col[(y * SS + sy) * n + x * SS + sx];
                    for k in 0..3 {
                        acc[k] += c[k];
                    }
                }
            }
            out.rgb[y * PANEL + x] = acc.map(|a| a / (SS * SS) as f32);
        }
    }
    // Panel border and LOD digit.
    for i in 0..PANEL {
        out.set(i as i64, 0, [0.5; 3]);
        out.set(0, i as i64, [0.5; 3]);
    }
    draw_text(
        &mut out,
        &format!("{label_lod}"),
        PANEL - 12,
        4,
        2,
        [0.1, 0.1, 0.1],
    );
    out
}

fn lerp3<const N: usize>(
    a: [f32; N],
    b: [f32; N],
    c: [f32; N],
    w0: f32,
    w1: f32,
    w2: f32,
) -> [f32; N] {
    std::array::from_fn(|k| a[k] * w0 + b[k] * w1 + c[k] * w2)
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    [v[0] / l, v[1] / l, v[2] / l]
}

/// 3x5 bitmap digits.
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

fn draw_text(c: &mut Canvas, s: &str, x: usize, y: usize, scale: usize, col: [f32; 3]) {
    for (i, ch) in s.chars().enumerate() {
        let Some(d) = ch.to_digit(10) else { continue };
        for (row, bits) in DIGITS[d as usize].iter().enumerate() {
            for b in 0..3 {
                if bits & (4 >> b) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            c.set(
                                (x + i * 4 * scale + b * scale + sx) as i64,
                                (y + row * scale + sy) as i64,
                                col,
                            );
                        }
                    }
                }
            }
        }
    }
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/flora-preview".to_string());
    std::fs::create_dir_all(&out).expect("output dir");
    let lib = species();
    let mut rows: Vec<Canvas> = Vec::new();
    let mut winter_rows: Vec<Canvas> = Vec::new();
    println!(
        "{:>2} {:<28} {:>6} {:>6} {:>6} {:>8} {:>8} {:>8} {:>9}",
        "#", "id", "tri0", "tri1", "tri2", "ms0", "ms1", "ms2", "tex ms"
    );
    for (i, s) in lib.iter().enumerate() {
        let t = Instant::now();
        let tex = textures(&s.params);
        let tex_ms = t.elapsed().as_secs_f64() * 1e3;
        write_png(
            &format!("{out}/atlas_{i:02}_{}.png", s.id),
            tex.atlas.width,
            tex.atlas.height,
            png::ColorType::Rgba,
            &tex.atlas.rgba,
        );
        let bark_both: Vec<u8> = tex
            .bark
            .rgba
            .chunks(tex.bark.width as usize * 4)
            .zip(tex.bark_normal.rgba.chunks(tex.bark.width as usize * 4))
            .flat_map(|(a, b)| a.iter().chain(b.iter()).copied())
            .collect();
        write_png(
            &format!("{out}/bark_{i:02}_{}.png", s.id),
            tex.bark.width * 2,
            tex.bark.height,
            png::ColorType::Rgba,
            &bark_both,
        );
        let mut meshes = Vec::new();
        let mut ms = Vec::new();
        for lod in Lod::ALL {
            // Median of 5 runs.
            let mut times = Vec::new();
            let mut m = None;
            for _ in 0..5 {
                let t = Instant::now();
                m = Some(generate(&s.params, 7, lod));
                times.push(t.elapsed().as_secs_f64() * 1e3);
            }
            times.sort_by(f64::total_cmp);
            ms.push(times[2]);
            meshes.push(m.unwrap());
        }
        println!(
            "{i:>2} {:<28} {:>6} {:>6} {:>6} {:>8.2} {:>8.2} {:>8.2} {:>9.1}",
            s.id,
            meshes[0].triangle_count(),
            meshes[1].triangle_count(),
            meshes[2].triangle_count(),
            ms[0],
            ms[1],
            ms[2],
            tex_ms
        );
        let mut row = Canvas::new(PANEL * 6, PANEL + LABEL_H, [1.0; 3]);
        draw_text(&mut row, &format!("{i}"), 4, 2, 2, [0.0; 3]);
        for (vi, view) in [View::Side, View::Top].into_iter().enumerate() {
            let frame = frame_for(&meshes[0], view);
            for (li, m) in meshes.iter().enumerate() {
                let p = render(m, &tex, view, frame, li);
                row.blit(&p, (vi * 3 + li) * PANEL, LABEL_H);
            }
        }
        rows.push(row);
        // Seasons for deciduous plants: January, April, July, October (LOD0 and LOD2 side).
        if s.params.deciduous {
            let mut wrow = Canvas::new(PANEL * 6, PANEL + LABEL_H, [1.0; 3]);
            draw_text(&mut wrow, &format!("{i}"), 4, 2, 2, [0.0; 3]);
            let frame = frame_for(&meshes[0], View::Side);
            for (k, (month, lod)) in [
                (1u8, Lod::Lod0),
                (4, Lod::Lod0),
                (7, Lod::Lod0),
                (10, Lod::Lod0),
                (11, Lod::Lod0),
                (1, Lod::Lod2),
            ]
            .into_iter()
            .enumerate()
            {
                let m = generate_in_season(&s.params, 7, lod, Season { month });
                let p = render(&m, &tex, View::Side, frame, month as usize);
                wrow.blit(&p, k * PANEL, LABEL_H);
            }
            winter_rows.push(wrow);
        }
    }
    let save_sheets = |rows: &[Canvas], name: &str| {
        for (k, chunk) in rows.chunks(ROWS_PER_SHEET).enumerate() {
            let mut sheet = Canvas::new(PANEL * 6, chunk.len() * (PANEL + LABEL_H), [1.0; 3]);
            for (r, row) in chunk.iter().enumerate() {
                sheet.blit(row, 0, r * (PANEL + LABEL_H));
            }
            sheet.save(&format!("{out}/{name}_{k:02}.png"));
        }
    };
    save_sheets(&rows, "sheet");
    save_sheets(&winter_rows, "seasons");

    // Hedge segments tiled three in a row, side and top.
    let mut hedge_rows = Vec::new();
    for (i, s) in lib
        .iter()
        .enumerate()
        .filter(|(_, s)| s.category == Category::Hedge)
    {
        let tex = textures(&s.params);
        let mut row = Canvas::new(PANEL * 6, PANEL + LABEL_H, [1.0; 3]);
        draw_text(&mut row, &format!("{i}"), 4, 2, 2, [0.0; 3]);
        for (li, lod) in Lod::ALL.into_iter().enumerate() {
            let mut joined = PlantMesh::default();
            for k in -1i32..=1 {
                let m = generate(&s.params, 8 + (k + 1) as u64, lod);
                let base = joined.positions.len() as u32;
                let ib = joined.indices.len() as u32;
                joined.positions.extend(
                    m.positions
                        .iter()
                        .map(|p| [p[0] + k as f32 * 1.0, p[1], p[2]]),
                );
                joined.normals.extend_from_slice(&m.normals);
                joined.uvs.extend_from_slice(&m.uvs);
                joined.wind.extend_from_slice(&m.wind);
                joined.indices.extend(m.indices.iter().map(|i| i + base));
                for p in &m.parts {
                    joined.parts.push(visioncraft_flora::Part {
                        material: p.material,
                        index_range: p.index_range.start + ib..p.index_range.end + ib,
                    });
                }
                joined.foliage_tint = m.foliage_tint;
            }
            for (vi, view) in [View::Side, View::Top].into_iter().enumerate() {
                let frame = frame_for(&joined, view);
                let p = render(&joined, &tex, view, frame, li);
                row.blit(&p, (vi * 3 + li) * PANEL, LABEL_H);
            }
        }
        hedge_rows.push(row);
    }
    save_sheets(&hedge_rows, "hedges_tiled");
    println!("\nLegend:");
    for (i, s) in lib.iter().enumerate() {
        println!(
            "{i:>2} {:<28} {} ({}), {:?}, {} x {} m",
            s.id, s.common_name, s.botanical_name, s.category, s.mature_height_m, s.mature_width_m
        );
    }
    println!("\nwrote previews to {out}");
}
