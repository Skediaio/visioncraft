//! Read-only commands the core can answer from the project alone. They never touch history.

use super::fail;
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
            &format!("What the design contains as numbers: per drawn shape its plan area, perimeter or length, extrusion height and volume, mesh surface and the surface per material; per planting its counts, assets, planted area and line length; and totals per material and per asset. A planting can also place lights or objects: `count` and `placements` count everything placed, `plants` only plant species (the built-in `flora:` ones and the project's own). Counts come from the planting evaluation on the terrain (height 0 without one). Shapes and plantings on a context (surroundings) layer are left out unless `ids` names them or `layer` is that layer. Lawn tufts are not counted, only placements the planting evaluation returns. {}", crate::quantities::CORE_NOTE),
            vec![
                param("ids", T::Array, "Only these shape and planting ids; an unknown id is an error."),
                param("layer", T::String, "Only the shapes and plantings on this layer id; an unknown layer is an error."),
            ],
            "{counted_with (plan|scene), shapes: [{id, name, layer, kind (area|extrusion), plan_area_m2, perimeter_m (closed) or length_m (open extrusion), height_m, volume_m3 (extrusions), surface_m2, materials: [{material (null: unassigned), name, area_m2}]}], plantings: [{id, name, layer, kind, count, plants, truncated, assets: [{asset, count}], area_m2 (area, brush, cluster), length_m (spline)}], materials: [{material, name, area_m2}], assets: [{asset, count}], totals: {plan_area_m2, surface_m2, volume_m3, plants, placements}}",
            json!({"layer": "shapes"}),
        )
        .read_only(),
        quantities_cmd,
    );
    s.register_with_spec(
        spec(
            "query.plants_for_light",
            "Plants for Light",
            "Ranks the built-in species by how well they suit the sun a spot gets. The spot is given as the hours of direct sun a day (`hours`; the plain command), or as a `point` or an `outline` whose hours the host measures with the same sun-hours computation as `query.sun_hours` (shadow from buildings, terrain, models and plant crowns): the window, the CLI and the MCP server do; a bare session has no geometry and refuses them. The date defaults to the summer solstice (the light ranges are summer-day hours): `season` or `date` change it (they, `cell_m`, `plants` and `years` only matter with a `point` or `outline`, and are ignored with `hours`), and `query.sun_hours` with `season: summer` gives the same hours. Light needs are estimated per species (full sun 6+ h, part shade 3 to 6 h, full shade under 3 h; many plants span two classes; the lawn wants 4+ h), not yet checked against open data. Fit is `full` when the hours lie inside the species' range and `partial` when outside it but within one class; species two classes away are left out. Over an outline the ranking is for the mean hours, with min and max reported. Full fits come first, the plants that want just this light before those that take anything.",
            vec![
                param("hours", T::Number, "Hours of direct sun a day, 0..24 (the plain command). Give exactly one of `hours`, `point` and `outline`.").unit("h"),
                param("point", T::Point2, "A spot [x, z] in metres; needs a host.").unit("m"),
                param("outline", T::Outline, "An area as [x, z] pairs in metres, at least 3; its hours are the mean over the area; needs a host.").unit("m"),
                param("season", T::String, "Which day the hours are for: 21 March, 21 June, 21 September or 21 December (six months on south of the equator).")
                    .default(json!("summer"))
                    .allowed(&["spring", "summer", "autumn", "winter"]),
                param("date", T::Object, "Instead of `season`: {month, day}, in the project's year."),
                param("grower", T::String, "Only this kind of plant.")
                    .allowed(&["tree", "conifer", "palm", "shrub", "clipped", "grass", "perennial", "ground_cover", "climber", "lawn"]),
                param("limit", T::Integer, "At most this many species are listed (1..200).").default(json!(25)),
                param("cell_m", T::Number, "Grid cell of an outline in metres, as in `query.sun_hours`.").unit("m").default(json!(1)),
                param("plants", T::Boolean, "Plant crowns cast shadow, as in `query.sun_hours`.").default(json!(true)),
                param("years", T::Number, "Years after planting the plants have grown, 0..60, as in `query.sun_hours`.").unit("years"),
            ],
            "{spot: {min, mean, max (hours) and, from a host, point or outline, date, cells}, hours (what the ranking is for), grower, matches, full, species: [{species, grower, fit (full|partial), reason, light: {min_hours, max_hours}, mature_height_m, mature_width_m, from_table, estimated, name (from a host)}], note}",
            json!({"hours": 2}),
        )
        .read_only(),
        plants_for_light_cmd,
    );
    s.register_with_spec(
        spec(
            "query.bloom_calendar",
            "Bloom Calendar",
            &format!("What flowers each month in the design: for every month of the year the species in flower (most plants first) and how many plants flower; the species with the months they flower in; the months with nothing in flower (`gaps`); and the species without bloom data (`unknown`). A species flowers in a month when its bloom months (written for the northern hemisphere) hold it, six months on south of the equator (`hemisphere`). The plants are those of the plantings: built-in `flora:` species and the project's own species; lights and objects a planting places are not counted. They are counted as `query.quantities` counts them. {}", crate::bloom::CORE_NOTE),
            vec![
                param("planting", T::String, "One planting's id (`g1`); by default every planting of the design. An unknown id is an error."),
                param("layer", T::String, "Only the plantings on this layer id, as `query.quantities` filters: an unknown layer is an error, and plantings on a context (surroundings) layer are left out unless `planting` names them or `layer` is that layer."),
            ],
            "{hemisphere (north|south), months: [{month, flowering: [{species, name?, plants, colour?}], plants, species}], species: [{species, name?, plants, bloom_months, colour?}], gaps: [month...], unknown: [{species, plants}]}",
            json!({}),
        )
        .read_only(),
        bloom_calendar_cmd,
    );
}

fn plants_for_light_cmd(_project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct P {
        hours: Option<f32>,
        point: Option<[f32; 2]>,
        outline: Option<Vec<[f32; 2]>>,
        season: Option<String>,
        date: Option<Value>,
        grower: Option<String>,
        limit: Option<usize>,
        cell_m: Option<f32>,
        plants: Option<bool>,
        years: Option<f32>,
    }
    let p: P = parse(params)?;
    if p.point.is_some() || p.outline.is_some() {
        return Err(bad(
            "`point` and `outline` need a host that casts shadow (the window, `visioncraft-cli` or the MCP server); a bare session has no geometry: give `hours`",
        ));
    }
    let hours = p
        .hours
        .ok_or_else(|| bad("give `hours` (or `point` or `outline` to a host)"))?;
    if !(0.0..=24.0).contains(&hours) {
        return Err(bad("hours: 0..24"));
    }
    let grower = parse_grower_param(p.grower.as_deref())?;
    let limit = p.limit.unwrap_or(25);
    if !(1..=200).contains(&limit) {
        return Err(bad("limit: 1..200"));
    }
    Ok(crate::plant_light::plants_for_light_json(
        json!({"min": hours, "mean": hours, "max": hours}),
        hours,
        grower,
        limit,
    ))
}

/// The grower filter of a command, refused with the names it knows.
fn parse_grower_param(name: Option<&str>) -> Result<Option<crate::growth::Grower>, CommandError> {
    name.map(|n| {
        crate::plant_light::parse_grower(n).ok_or_else(|| {
            bad(&format!(
                "grower: `{n}` is not one of tree, conifer, palm, shrub, clipped, grass, perennial, ground_cover, climber, lawn"
            ))
        })
    })
    .transpose()
}

fn bloom_calendar_cmd(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        planting: Option<String>,
        layer: Option<String>,
    }
    let p: P = parse(params)?;
    let filter = crate::quantities::Filter {
        ids: p.planting.as_ref().map(std::slice::from_ref),
        layer: p.layer.as_deref(),
    };
    // The core alone knows no species: everything is `unknown`; a host answers with the
    // species library.
    crate::bloom::calendar(project, &filter, None, &|_| None).map_err(fail)
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
