//! Headless verification mode: `visioncraft --smoke script.json --out dir` runs a list of steps
//! against the same scene sync and renderer as the app and writes PNGs.
//!
//! A script is a JSON array (or `{"steps": [...]}`). A step is a string
//! (`"load model path [name N] [at x y z] [rot deg] [scale s]"`, `"reimport m1 path"`,
//! `"set camera orbit yaw pitch dist px py pz"`, `"set camera look ex ey ez tx ty tz"`,
//! `"set camera walk x z heading_deg pitch_deg"`, `"set camera top cx cz half_height"`,
//! `"set camera frame"`, `"set quality low|medium|high"`, `"set exposure stops"`,
//! `"set clay on|off"`, `"highlight owner face-name"`, `"highlight none"`,
//! `"screenshot name w h"`) or an object `{"run": "command.name", "params": {...}}`.

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Cam {
    Orbit {
        yaw: f32,
        pitch: f32,
        dist: f32,
        pivot: [f32; 3],
    },
    Look {
        eye: [f32; 3],
        target: [f32; 3],
    },
    Walk {
        x: f32,
        z: f32,
        heading_deg: f32,
        pitch_deg: f32,
    },
    Top {
        cx: f32,
        cz: f32,
        half: f32,
    },
    Frame,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Run {
        name: String,
        params: Value,
    },
    LoadModel {
        path: String,
        name: Option<String>,
        at: [f32; 3],
        rot: f32,
        scale: f32,
    },
    Reimport {
        id: String,
        path: String,
    },
    Camera(Cam),
    Quality(String),
    Exposure(f32),
    Clay(bool),
    Highlight(Option<(String, String)>),
    Faces(String),
    Pick {
        x: f32,
        y: f32,
    },
    /// `{"plant": {...}}`: see `planting_smoke`.
    Plant(Value),
    Screenshot {
        name: String,
        w: u32,
        h: u32,
    },
}

fn nums<const N: usize>(t: &[&str]) -> Result<[f32; N], String> {
    if t.len() != N {
        return Err(format!("expected {N} numbers, got {}", t.len()));
    }
    let mut out = [0.0; N];
    for (o, s) in out.iter_mut().zip(t) {
        *o = s.parse().map_err(|_| format!("`{s}` is not a number"))?;
    }
    Ok(out)
}

pub fn parse_step(v: &Value) -> Result<Step, String> {
    if let Some(obj) = v.as_object() {
        if let Some(p) = obj.get("plant") {
            return Ok(Step::Plant(p.clone()));
        }
        let name = obj
            .get("run")
            .and_then(Value::as_str)
            .ok_or("an object step needs `run`")?;
        return Ok(Step::Run {
            name: name.into(),
            params: obj
                .get("params")
                .cloned()
                .unwrap_or(Value::Object(Default::default())),
        });
    }
    let s = v.as_str().ok_or("a step is a string or an object")?;
    let t: Vec<&str> = s.split_whitespace().collect();
    match t.as_slice() {
        ["load", "model", path, rest @ ..] => {
            let (mut name, mut at, mut rot, mut scale) = (None, [0.0; 3], 0.0, 1.0);
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "name" => name = rest.get(i + 1).map(|s| s.to_string()),
                    "at" => at = nums::<3>(rest.get(i + 1..i + 4).ok_or("`at` needs x y z")?)?,
                    "rot" => {
                        rot = nums::<1>(rest.get(i + 1..i + 2).ok_or("`rot` needs degrees")?)?[0]
                    }
                    "scale" => {
                        scale =
                            nums::<1>(rest.get(i + 1..i + 2).ok_or("`scale` needs a factor")?)?[0]
                    }
                    other => return Err(format!("unknown load option `{other}`")),
                }
                i += if rest[i] == "at" { 4 } else { 2 };
            }
            Ok(Step::LoadModel {
                path: path.to_string(),
                name,
                at,
                rot,
                scale,
            })
        }
        ["reimport", id, path] => Ok(Step::Reimport {
            id: id.to_string(),
            path: path.to_string(),
        }),
        ["set", "camera", "frame"] => Ok(Step::Camera(Cam::Frame)),
        ["set", "camera", "orbit", n @ ..] => {
            let [yaw, pitch, dist, x, y, z] = nums::<6>(n)?;
            Ok(Step::Camera(Cam::Orbit {
                yaw: yaw.to_radians(),
                pitch: pitch.to_radians(),
                dist,
                pivot: [x, y, z],
            }))
        }
        ["set", "camera", "look", n @ ..] => {
            let [a, b, c, d, e, f] = nums::<6>(n)?;
            Ok(Step::Camera(Cam::Look {
                eye: [a, b, c],
                target: [d, e, f],
            }))
        }
        ["set", "camera", "walk", n @ ..] => {
            let [x, z, heading_deg, pitch_deg] = nums::<4>(n)?;
            Ok(Step::Camera(Cam::Walk {
                x,
                z,
                heading_deg,
                pitch_deg,
            }))
        }
        ["set", "camera", "top", n @ ..] => {
            let [cx, cz, half] = nums::<3>(n)?;
            Ok(Step::Camera(Cam::Top { cx, cz, half }))
        }
        ["set", "quality", q] if ["low", "medium", "high"].contains(q) => {
            Ok(Step::Quality(q.to_string()))
        }
        ["set", "exposure", n] => Ok(Step::Exposure(nums::<1>(&[n])?[0])),
        ["set", "clay", on @ ("on" | "off")] => Ok(Step::Clay(*on == "on")),
        ["pick", x, y] => Ok(Step::Pick {
            x: x.parse().map_err(|_| "x")?,
            y: y.parse().map_err(|_| "y")?,
        }),
        ["faces", owner] => Ok(Step::Faces(owner.to_string())),
        ["highlight", "none"] => Ok(Step::Highlight(None)),
        ["highlight", owner, face] => {
            Ok(Step::Highlight(Some((owner.to_string(), face.to_string()))))
        }
        ["screenshot", name, w, h] => Ok(Step::Screenshot {
            name: name.to_string(),
            w: w.parse().map_err(|_| "width is not a number")?,
            h: h.parse().map_err(|_| "height is not a number")?,
        }),
        _ => Err(format!("unknown step `{s}`")),
    }
}

pub fn parse_script(text: &str) -> Result<Vec<Step>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("script is not JSON: {e}"))?;
    let list = match &v {
        Value::Array(a) => a,
        Value::Object(o) => o
            .get("steps")
            .and_then(Value::as_array)
            .ok_or("expected a `steps` array")?,
        _ => return Err("expected an array of steps".into()),
    };
    list.iter()
        .enumerate()
        .map(|(i, s)| parse_step(s).map_err(|e| format!("step {}: {e}", i + 1)))
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::run;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{Cam, Step, parse_script};
    use crate::camera::View;
    use crate::studio::{Readback, Source, Studio};
    use glam::{Vec2, Vec3};
    use std::path::Path;
    use std::sync::Arc;
    use visioncraft_core::{ModelKind, Project};
    use visioncraft_render::{Quality, Renderer};

    fn kind_of(path: &str) -> Result<ModelKind, String> {
        match Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .as_deref()
        {
            Some("3dcraft") => Ok(ModelKind::Craft3d),
            Some("obj") => Ok(ModelKind::Obj),
            _ => Err(format!("`{path}` is neither .3dcraft nor .obj")),
        }
    }

    fn write_png(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .and_then(|mut w| w.write_image_data(rgba))
            .map_err(|e| e.to_string())
    }

    pub fn run(script: &Path, out: &Path) -> Result<(), String> {
        let steps = parse_script(
            &std::fs::read_to_string(script).map_err(|e| format!("{}: {e}", script.display()))?,
        )?;
        std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
        let (device, queue) = pollster::block_on(visioncraft_render::headless_device())?;
        let renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let mut s = Studio::new(renderer, Project::default());
        let mut plant = crate::planting_smoke::PlantSmoke::default();
        for (i, step) in steps.into_iter().enumerate() {
            let at = |e: String| format!("step {}: {e}", i + 1);
            match step {
                Step::Run { name, params } => {
                    s.session
                        .run(&name, params)
                        .map_err(|e| at(format!("{name}: {e}")))?;
                }
                Step::LoadModel {
                    path,
                    name,
                    at: pos,
                    rot,
                    scale,
                } => {
                    let bytes = std::fs::read(&path).map_err(|e| at(format!("{path}: {e}")))?;
                    let stem = Path::new(&path)
                        .file_stem()
                        .and_then(|f| f.to_str())
                        .unwrap_or("model")
                        .to_string();
                    let src = Source {
                        bytes: Arc::from(bytes),
                        path: Some(path.clone().into()),
                    };
                    let (id, warnings) = s
                        .add_model(
                            &name.unwrap_or(stem),
                            kind_of(&path).map_err(at)?,
                            src,
                            &path,
                        )
                        .map_err(at)?;
                    s.person_near(&id);
                    for w in warnings {
                        eprintln!("warning: {w}");
                    }
                    s.session
                        .run("model.set_transform", serde_json::json!({"id": id, "translation": pos, "rotation_y_deg": rot, "scale": scale}))
                        .map_err(|e| at(e.to_string()))?;
                }
                Step::Reimport { id, path } => {
                    let bytes = std::fs::read(&path).map_err(|e| at(format!("{path}: {e}")))?;
                    let (w, dangling) = s
                        .reimport(
                            &id,
                            Source {
                                bytes: Arc::from(bytes),
                                path: Some(path.into()),
                            },
                        )
                        .map_err(at)?;
                    eprintln!(
                        "reimported {id}: {} warnings, {dangling} assignments without a face",
                        w.len()
                    );
                }
                Step::Camera(c) => {
                    s.sync();
                    match c {
                        Cam::Frame => {
                            s.frame_all();
                            eprintln!("scene bounds {:?}", s.scene.bounds());
                        }
                        Cam::Orbit {
                            yaw,
                            pitch,
                            dist,
                            pivot,
                        } => {
                            s.camera.view = View::Orbit;
                            (s.camera.yaw, s.camera.pitch, s.camera.dist, s.camera.pivot) =
                                (yaw, pitch, dist, Vec3::from(pivot));
                        }
                        Cam::Look { eye, target } => {
                            s.camera.look_from(Vec3::from(eye), Vec3::from(target))
                        }
                        Cam::Walk {
                            x,
                            z,
                            heading_deg,
                            pitch_deg,
                        } => {
                            let g = s.scene.floor_below(x, 100.0, z).unwrap_or(0.0);
                            s.camera.view = View::Walk;
                            s.camera.walk = crate::camera::Walk {
                                eye: Vec3::new(x, g + crate::camera::EYE_HEIGHT, z),
                                yaw: heading_deg.to_radians(),
                                pitch: pitch_deg.to_radians(),
                                ground: g,
                            };
                        }
                        Cam::Top { cx, cz, half } => {
                            s.camera.view = View::Top;
                            (s.camera.top_center, s.camera.top_half_height) =
                                (Vec2::new(cx, cz), half);
                        }
                    }
                }
                Step::Quality(q) => {
                    s.quality = Quality::PRESETS
                        .iter()
                        .position(|(n, _)| *n == q)
                        .unwrap_or(1)
                }
                Step::Exposure(e) => s.exposure = e,
                Step::Clay(c) => {
                    s.session
                        .run("style.set", serde_json::json!({"clay": c}))
                        .map_err(|e| at(e.to_string()))?;
                }
                Step::Pick { x, y } => {
                    s.sync();
                    let size = Vec2::new(s.size[0] as f32, s.size[1] as f32);
                    match s.pick(Vec2::new(x, y), size, false) {
                        Some(p) => println!(
                            "pick {x},{y}: {} {} (body {}) at {:?}",
                            p.owner, p.face_name, p.body, p.point
                        ),
                        None => println!("pick {x},{y}: nothing"),
                    }
                }
                Step::Plant(v) => {
                    crate::planting_smoke::run(&mut s, &mut plant, &device, &queue, &v)
                        .map_err(at)?
                }
                Step::Faces(owner) => {
                    s.sync();
                    let g = s
                        .scene
                        .geometry(&owner)
                        .ok_or_else(|| at(format!("no owner `{owner}`")))?;
                    for (i, f) in g.faces.iter().enumerate() {
                        let tris = g
                            .triangle_faces
                            .iter()
                            .filter(|t| **t as usize == i)
                            .count();
                        println!(
                            "face {i}: {} (body {}, {tris} triangles)",
                            f.name, g.bodies[f.body as usize].id
                        );
                    }
                }
                Step::Highlight(h) => {
                    s.sync();
                    s.highlight = match h {
                        None => None,
                        Some((owner, face)) => {
                            let g = s
                                .scene
                                .geometry(&owner)
                                .ok_or_else(|| at(format!("no owner `{owner}`")))?;
                            let f = g
                                .faces
                                .iter()
                                .position(|f| f.name == face)
                                .ok_or_else(|| at(format!("no face `{face}`")))?;
                            Some((owner, f))
                        }
                    }
                }
                Step::Screenshot { name, w, h } => {
                    s.resize(w, h);
                    // plants are generated one step per frame: let them finish
                    for _ in 0..600 {
                        s.prepare();
                        if s.plants_pending() == 0 {
                            break;
                        }
                    }
                    s.prepare();
                    let rb = Readback::start(&mut s, &device, &queue);
                    device
                        .poll(wgpu::PollType::wait_indefinitely())
                        .map_err(|e| at(e.to_string()))?;
                    let pixels = rb
                        .take(&device)
                        .ok_or_else(|| at("readback failed".into()))?;
                    let file = out.join(format!("{name}.png"));
                    write_png(&file, &pixels, w, h).map_err(at)?;
                    println!("wrote {}", file.display());
                }
            }
            for p in s.problems.drain(..) {
                eprintln!("problem: {p}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_every_step_kind() {
        let script = json!([
            "load model a.3dcraft name Villa at 1 2 3 rot 90 scale 2",
            {"run": "time.set", "params": {"hour": 15.5}},
            "set camera orbit 30 20 25 0 1 0",
            "set camera walk 1 8 180 -3",
            "set camera frame",
            "set quality high",
            "highlight m1 F2:endCap(e5,e6,e7,e8)",
            "screenshot villa 960 540",
            "reimport m1 b.3dcraft"
        ]);
        let steps = parse_script(&script.to_string()).unwrap();
        assert_eq!(steps.len(), 9);
        assert_eq!(
            steps[0],
            Step::LoadModel {
                path: "a.3dcraft".into(),
                name: Some("Villa".into()),
                at: [1.0, 2.0, 3.0],
                rot: 90.0,
                scale: 2.0
            }
        );
        assert_eq!(
            steps[7],
            Step::Screenshot {
                name: "villa".into(),
                w: 960,
                h: 540
            }
        );
        assert!(
            matches!(&steps[6], Step::Highlight(Some((o, f))) if o == "m1" && f.starts_with("F2:"))
        );
    }

    #[test]
    fn errors_name_the_step() {
        let e = parse_script(r#"["screenshot a 1 1", "dance"]"#).unwrap_err();
        assert!(e.starts_with("step 2:"), "{e}");
        assert!(parse_script(r#"["set camera orbit 1 2"]"#).is_err());
        assert!(parse_script("{").is_err());
    }
}
