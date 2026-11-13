//! Plantings in the headless smoke script. A step is `{"plant": {"op": ..., ...}}`; the ops call
//! the same functions as the tools do (`Form::params` through `commit` and `commit_stroke`), so a
//! script exercises what a click does.
//!
//! * `add`: `tool` (`single`, `line`, `cluster`, `area`, `surface`, `brush`), the settings
//!   (`palette` as ids, `[id, weight]` pairs or `{asset, weight}`, or `preset` with a preset id and
//!   optional `layer_density_scale`; `density`, `min_distance`, `scale: [min, max]`, `spin`,
//!   `seed`, `layer`, `jitter`, `pos_jitter`, `spacing: {count: n}` or `{every: m}`, `offset`,
//!   `closed`, `count`, `radius`, `rotation`, `max_slope`) and the geometry (`position`, `points`,
//!   `center`, `owner` with `faces` or `top_faces: true`, `strokes: [{points, radius, erase}]`).
//! * `stroke`: `points`, `radius`, `erase`, the settings above, `new: true` for a new brush;
//!   consecutive strokes with the same settings extend one brush.
//! * `sync`, `bench` (`frames`, `w`, `h`), `pick` (`x`, `y`), `duplicate` (`id`).

use crate::planting_tools::{BrushTarget, Form, Geom, commit};
use serde_json::Value;
use visioncraft_core::planting::{Spacing, Stroke, Weighted};
use visioncraft_core::planting_presets;

#[derive(Default)]
pub struct PlantSmoke {
    pub brush: BrushTarget,
}

fn num(v: &Value, key: &str) -> Option<f32> {
    v.get(key).and_then(Value::as_f64).map(|n| n as f32)
}

fn pair(v: &Value) -> Option<[f32; 2]> {
    Some([v.get(0)?.as_f64()? as f32, v.get(1)?.as_f64()? as f32])
}

fn points(v: &Value) -> Result<Vec<[f32; 2]>, String> {
    v.as_array()
        .ok_or("`points` is a list of [x, z]")?
        .iter()
        .map(|p| pair(p).ok_or_else(|| "a point is [x, z]".to_string()))
        .collect()
}

/// The settings of a step.
pub fn form_from_json(v: &Value) -> Result<Form, String> {
    let mut f = Form::default();
    if let Some(id) = v.get("preset").and_then(Value::as_str) {
        let p = planting_presets()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("no preset `{id}`"))?;
        f.layers = p.layers;
        f.preset = Some(p.name);
        let k = num(v, "layer_density_scale").unwrap_or(1.0);
        f.layers.iter_mut().for_each(|l| l.density_per_m2 *= k);
    }
    if let Some(a) = v.get("palette").and_then(Value::as_array) {
        f.palette = a
            .iter()
            .map(|e| match e {
                Value::String(s) => Ok(Weighted {
                    asset: s.clone(),
                    weight: 1.0,
                }),
                Value::Array(p) => Ok(Weighted {
                    asset: p.first().and_then(Value::as_str).unwrap_or_default().into(),
                    weight: p.get(1).and_then(Value::as_f64).unwrap_or(1.0) as f32,
                }),
                other => serde_json::from_value(other.clone()).map_err(|e| e.to_string()),
            })
            .collect::<Result<_, String>>()?;
    }
    let set = |dst: &mut f32, key: &str| {
        if let Some(n) = num(v, key) {
            *dst = n;
        }
    };
    set(&mut f.density, "density");
    set(&mut f.min_distance, "min_distance");
    set(&mut f.jitter, "jitter");
    set(&mut f.pos_jitter, "pos_jitter");
    set(&mut f.offset, "offset");
    set(&mut f.rotation, "rotation");
    set(&mut f.radius, "radius");
    set(&mut f.max_slope, "max_slope");
    if let Some([a, b]) = v.get("scale").and_then(pair) {
        (f.scale_min, f.scale_max) = (a, b);
    }
    if let Some(b) = v.get("spin").and_then(Value::as_bool) {
        f.spin = b;
    }
    if let Some(b) = v.get("closed").and_then(Value::as_bool) {
        f.closed = b;
    }
    if let Some(n) = v.get("seed").and_then(Value::as_u64) {
        f.seed = n;
    }
    if let Some(n) = v.get("count").and_then(Value::as_u64) {
        f.count = n as u32;
    }
    if let Some(l) = v.get("layer").and_then(Value::as_str) {
        f.layer = l.into();
    }
    if let Some(s) = v.get("spacing") {
        match serde_json::from_value::<Spacing>(s.clone()).map_err(|e| e.to_string())? {
            Spacing::Count(n) => (f.by_count, f.count) = (true, n),
            Spacing::Every(d) => (f.by_count, f.every) = (false, d),
        }
    }
    Ok(f)
}

fn strokes(v: &Value) -> Result<Vec<Stroke>, String> {
    v.as_array()
        .ok_or("`strokes` is a list")?
        .iter()
        .map(|s| {
            Ok(Stroke {
                points: points(s.get("points").ok_or("a stroke needs `points`")?)?,
                radius_m: num(s, "radius").ok_or("a stroke needs `radius`")?,
                erase: s.get("erase").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

/// The geometry a tool's gesture would produce; `top_faces` is resolved by the caller.
pub fn geom_from_json(tool: &str, v: &Value, faces: Vec<String>) -> Result<Geom, String> {
    Ok(match tool {
        "single" => {
            let p = v.get("position").ok_or("`single` needs `position`")?;
            let at = match p.as_array().map(Vec::len) {
                Some(3) => [
                    p[0].as_f64().unwrap_or(0.0) as f32,
                    p[1].as_f64().unwrap_or(0.0) as f32,
                    p[2].as_f64().unwrap_or(0.0) as f32,
                ],
                _ => {
                    let q = pair(p).ok_or("`position` is [x, z] or [x, y, z]")?;
                    [q[0], 0.0, q[1]]
                }
            };
            Geom::Single {
                position: at,
                rotation_deg: num(v, "rotation").unwrap_or(0.0),
            }
        }
        "line" => Geom::Line(points(v.get("points").ok_or("`line` needs `points`")?)?),
        "area" => Geom::Area(points(v.get("points").ok_or("`area` needs `points`")?)?),
        "cluster" => Geom::Cluster {
            center: v
                .get("center")
                .and_then(pair)
                .ok_or("`cluster` needs `center`")?,
            radius: num(v, "radius").unwrap_or(2.0),
        },
        "surface" => Geom::Surface {
            owner: v
                .get("owner")
                .and_then(Value::as_str)
                .ok_or("`surface` needs `owner`")?
                .into(),
            faces: if faces.is_empty() {
                v.get("faces")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|f| f.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                faces
            },
        },
        "brush" => Geom::Brush(strokes(v.get("strokes").ok_or("`brush` needs `strokes`")?)?),
        other => return Err(format!("unknown tool `{other}`")),
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::run;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use crate::plant_scene::now_ms;
    use crate::planting_tools::commit_stroke;
    use crate::studio::Studio;
    use visioncraft_core::ImportedModel;

    /// Faces of a mesh whose triangles all point up: a lawn's top.
    fn top_faces(g: &ImportedModel) -> Vec<String> {
        let mut up = vec![true; g.faces.len()];
        for (t, tri) in g.triangles.iter().enumerate() {
            let [a, b, c] = tri.map(|i| g.positions[i as usize]);
            let (u, w) = (
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            let n = [
                u[1] * w[2] - u[2] * w[1],
                u[2] * w[0] - u[0] * w[2],
                u[0] * w[1] - u[1] * w[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
            if n[1] / len < 0.9 {
                up[g.triangle_faces[t] as usize] = false;
            }
        }
        g.faces
            .iter()
            .zip(up)
            .filter(|(_, u)| *u)
            .map(|(f, _)| f.name.clone())
            .collect()
    }

    fn report(s: &Studio, what: &str, ms: f64) {
        let t = s.scene.timings;
        println!(
            "plant {what}: {} placements, evaluate {:.1} ms, upload {:.1} ms, sync {ms:.1} ms",
            t.placements, t.eval_ms, t.upload_ms
        );
    }

    pub fn run(
        s: &mut Studio,
        st: &mut PlantSmoke,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        v: &Value,
    ) -> Result<(), String> {
        let op = v
            .get("op")
            .and_then(Value::as_str)
            .ok_or("a plant step needs `op`")?;
        match op {
            "add" => {
                let tool = v
                    .get("tool")
                    .and_then(Value::as_str)
                    .ok_or("`add` needs `tool`")?;
                let form = form_from_json(v)?;
                s.sync(); // the ground the placements stand on
                let faces = match (tool, v.get("top_faces").and_then(Value::as_bool)) {
                    ("surface", Some(true)) => {
                        let owner = v.get("owner").and_then(Value::as_str).unwrap_or_default();
                        s.scene
                            .geometry(owner)
                            .map(|g| top_faces(g))
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                let geom = geom_from_json(tool, v, faces)?;
                let name = v.get("name").and_then(Value::as_str).unwrap_or(tool);
                let id = commit(&mut s.session, &form, &geom, name)?;
                let t0 = now_ms();
                s.sync();
                println!("plant add {tool} -> {id}");
                report(s, tool, now_ms() - t0);
            }
            "stroke" => {
                if v.get("new").and_then(Value::as_bool) == Some(true) {
                    st.brush = None;
                }
                let form = form_from_json(v)?;
                let stroke = Stroke {
                    points: points(v.get("points").ok_or("`stroke` needs `points`")?)?,
                    radius_m: num(v, "radius").ok_or("`stroke` needs `radius`")?,
                    erase: v.get("erase").and_then(Value::as_bool).unwrap_or(false),
                };
                s.sync();
                let id = commit_stroke(&mut s.session, &mut st.brush, &form, stroke, "Brush")?;
                let t0 = now_ms();
                s.sync();
                println!("plant stroke -> {id}");
                report(s, "stroke", now_ms() - t0);
            }
            "sync" => {
                let t0 = now_ms();
                s.sync();
                report(s, "sync", now_ms() - t0);
            }
            "duplicate" => {
                let id = v
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("`duplicate` needs `id`")?;
                let g = s
                    .project()
                    .generators
                    .iter()
                    .find(|g| g.id() == id)
                    .ok_or_else(|| format!("no generator `{id}`"))?;
                let (cmd, p) = crate::handles::duplicate_params(g, [2.0, 2.0], 4242);
                let r = s.session.run(cmd, p).map_err(|e| e.to_string())?;
                println!("plant duplicate {id} -> {}", r["id"]);
            }
            "pick" => {
                s.sync();
                let (x, y) = (num(v, "x").unwrap_or(0.0), num(v, "y").unwrap_or(0.0));
                let size = glam::Vec2::new(s.size[0] as f32, s.size[1] as f32);
                let (o, d) = s.pointer_ray(glam::Vec2::new(x, y), size);
                match s.scene.pick_plant(o, d) {
                    Some((id, t)) => println!("plant pick {x},{y}: {id} at {t:.2} m"),
                    None => println!("plant pick {x},{y}: nothing"),
                }
            }
            "bench" => {
                let (w, h) = (
                    v.get("w").and_then(Value::as_u64).unwrap_or(1280) as u32,
                    v.get("h").and_then(Value::as_u64).unwrap_or(720) as u32,
                );
                let frames = v.get("frames").and_then(Value::as_u64).unwrap_or(60) as usize;
                s.resize(w, h);
                for _ in 0..2000 {
                    s.prepare();
                    if s.plants_pending() == 0 {
                        break;
                    }
                }
                let mut each = Vec::new();
                for _ in 0..frames + 3 {
                    let t0 = now_ms();
                    s.prepare();
                    let mut enc = device.create_command_encoder(&Default::default());
                    s.renderer.render(&mut enc);
                    queue.submit([enc.finish()]);
                    device
                        .poll(wgpu::PollType::wait_indefinitely())
                        .map_err(|e| e.to_string())?;
                    each.push(now_ms() - t0);
                }
                let each = &each[3..]; // the first frames build pipelines
                let avg = each.iter().sum::<f64>() / each.len() as f64;
                let max = each.iter().copied().fold(0.0, f64::max);
                println!(
                    "plant bench {w}x{h}: {} frames, average {avg:.1} ms ({:.1} fps), worst {max:.1} ms",
                    each.len(),
                    1000.0 / avg
                );
            }
            other => return Err(format!("unknown plant op `{other}`")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_and_geometry_parse() {
        let v = json!({
            "preset": "mixed-woodland", "layer_density_scale": 0.5, "scale": [0.9, 1.1],
            "spacing": {"count": 7}, "seed": 9, "palette": ["flora:quercus-robur", ["flora:taxus-baccata", 3]],
        });
        let f = form_from_json(&v).unwrap();
        assert_eq!(f.layers.len(), 3);
        assert_eq!(
            f.layers[0].density_per_m2,
            planting_presets()[2].layers[0].density_per_m2 * 0.5
        );
        assert!(f.by_count && f.count == 7 && f.seed == 9);
        assert_eq!(f.palette[1].weight, 3.0);
        assert!(form_from_json(&json!({"preset": "nope"})).is_err());
        let g = geom_from_json(
            "brush",
            &json!({"strokes": [{"points": [[0, 0], [5, 0]], "radius": 3}]}),
            vec![],
        )
        .unwrap();
        assert!(matches!(g, Geom::Brush(s) if s.len() == 1 && s[0].radius_m == 3.0));
        assert!(geom_from_json("line", &json!({}), vec![]).is_err());
        assert!(geom_from_json("pond", &json!({}), vec![]).is_err());
    }
}
