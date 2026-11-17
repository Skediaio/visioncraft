//! Read-only commands the core can answer from the project alone. They never touch history.

use crate::measure;
use crate::project::{Project, SunTime};
use crate::session::{CommandError, Session, bad, parse};
use crate::spec::{ParamType as T, param, spec};
use crate::sun::{sun_position, sunrise_sunset};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "query.sun",
            "Sun Position",
            "Where the sun stands for the project's site and time (or the time given), with sunrise and sunset that day.",
            vec![param(
                "time",
                T::Object,
                "Another moment {year, month, day, hour}; parts left out are taken from the project's time.",
            )],
            "{azimuth_deg (true compass bearing, clockwise from north), elevation_deg, direction ([x, y, z] unit vector towards the sun in drawing space: +X east, -Z north, turned by site.north_deg), sunrise_hour, sunset_hour (null in polar day or night), above_horizon}",
            json!({"time": {"hour": 21}}),
        )
        .read_only(),
        sun,
    );
    s.register_with_spec(
        spec(
            "query.measure",
            "Measure",
            "Measures points or an outline: path length, polygon area, height difference between the first and last point, or slope.",
            vec![
                param("kind", T::String, "What to measure: `length` of the polyline through `points` (m), `area` of `outline` (m2), `height_difference` last minus first point (m, negative downhill), `slope` first to last in percent (null when straight up).")
                    .required()
                    .allowed(&["length", "area", "height_difference", "slope"]),
                param("points", T::Points3, "[x, y, z] points in metres, at least 2; for length, height_difference and slope."),
                param("outline", T::Outline, "[x, z] polygon in metres, at least 3, any winding; for area."),
            ],
            "{kind, value, unit}",
            json!({"kind": "length", "points": [[0, 0, 0], [3, 0, 4]]}),
        )
        .read_only(),
        measure_cmd,
    );
    s.register_with_spec(
        spec(
            "query.quantities",
            "Quantities",
            "What the design contains as numbers: per drawn shape its plan area, perimeter or length, extrusion height and volume, mesh surface and the surface per material; per planting its counts, assets, planted area and line length; and totals per material and per asset. A planting can also place lights or objects: `count` and `placements` count everything placed, `plants` only plant species (`flora:`). Counts come from the planting evaluation on the terrain (height 0 without one), without the renderer's building rule, so plants under buildings are still counted. Shapes and plantings on a context (surroundings) layer are left out unless `ids` names them or `layer` is that layer. Linked models (3dcraft, OBJ) have no geometry in the core and are left out; lawn tufts are not counted, only placements the planting evaluation returns.",
            vec![
                param("ids", T::Array, "Only these shape and planting ids; an unknown id is an error."),
                param("layer", T::String, "Only the shapes and plantings on this layer id; an unknown layer is an error."),
            ],
            "{shapes: [{id, name, layer, kind (area|extrusion), plan_area_m2, perimeter_m (closed) or length_m (open extrusion), height_m, volume_m3 (extrusions), surface_m2, materials: [{material (null: unassigned), name, area_m2}]}], plantings: [{id, name, layer, kind, count, plants, truncated, assets: [{asset, count}], area_m2 (area, brush, cluster), length_m (spline)}], materials: [{material, name, area_m2}], assets: [{asset, count}], totals: {plan_area_m2, surface_m2, volume_m3, plants, placements}}",
            json!({"layer": "shapes"}),
        )
        .read_only(),
        quantities_cmd,
    );
}

fn quantities_cmd(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        ids: Option<Vec<String>>,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    let filter = crate::quantities::Filter {
        ids: p.ids.as_deref(),
        layer: p.layer.as_deref(),
    };
    crate::quantities::quantities(project, &filter).map_err(|e| bad(&e))
}

fn sun(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let mut time = serde_json::to_value(project.time).expect("a time serialises");
    if let Some(given) = params.get("time").and_then(Value::as_object) {
        let fields = time.as_object_mut().expect("a time is an object");
        for (k, v) in given {
            if !fields.contains_key(k) {
                return Err(bad(&format!(
                    "`time` has no field `{k}` (year, month, day, hour)"
                )));
            }
            fields.insert(k.clone(), v.clone());
        }
    }
    let time: SunTime = parse(&time)?;
    let p = sun_position(&project.site, &time);
    let (rise, set) =
        sunrise_sunset(&project.site, &time).map_or((None, None), |(r, s)| (Some(r), Some(s)));
    Ok(json!({
        "azimuth_deg": p.azimuth_deg, "elevation_deg": p.elevation_deg, "direction": p.direction,
        "sunrise_hour": rise, "sunset_hour": set, "above_horizon": p.elevation_deg > 0.0,
    }))
}

fn measure_cmd(_: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        kind: String,
        points: Option<Vec<[f32; 3]>>,
        outline: Option<Vec<[f32; 2]>>,
    }
    let p: P = parse(params)?;
    let points = |min: usize| match &p.points {
        Some(pts) if pts.len() >= min => Ok(pts.clone()),
        _ => Err(bad(&format!(
            "`{}` needs `points` with at least {min} points",
            p.kind
        ))),
    };
    let (value, unit): (Value, &str) = match p.kind.as_str() {
        "length" => (measure::polyline_length(&points(2)?).into(), "m"),
        "area" => match &p.outline {
            Some(o) if o.len() >= 3 => (measure::polygon_area(o).into(), "m2"),
            _ => return Err(bad("`area` needs `outline` with at least 3 points")),
        },
        "height_difference" => {
            let pts = points(2)?;
            (
                measure::height_difference(pts[0], pts[pts.len() - 1]).into(),
                "m",
            )
        }
        "slope" => {
            let pts = points(2)?;
            (
                measure::slope_percent(pts[0], pts[pts.len() - 1]).into(),
                "%",
            )
        }
        other => return Err(bad(&format!("unknown kind `{other}`"))),
    };
    Ok(json!({"kind": p.kind, "value": value, "unit": unit}))
}
