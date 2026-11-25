//! What both protocols run: every command of the core's [`Session`], plus the host's own
//! (files, history, inspect, queries, pictures), one registry with one listing, and batches
//! that are all-or-nothing.

use crate::MAX_BATCH_STEPS;
use crate::docs::{CommandDoc, ParamDoc, from_spec};
use crate::fields::{Fields, describe, shorten};
use crate::host::{Host, PhotoRequest, RenderRequest, missing_json};
use crate::inspect::{self, Verbosity};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use visioncraft_core::{ModelKind, Project, Session};

/// Wall-clock milliseconds to a tenth; `null` where there is no clock (the browser).
pub(crate) struct Stopwatch(#[cfg(not(target_arch = "wasm32"))] std::time::Instant);

impl Stopwatch {
    pub fn start() -> Self {
        Self(
            #[cfg(not(target_arch = "wasm32"))]
            std::time::Instant::now(),
        )
    }

    pub fn ms(&self) -> Value {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let ms = self.0.elapsed().as_secs_f64() * 1000.0;
            json!((ms * 10.0).round() / 10.0)
        }
        #[cfg(target_arch = "wasm32")]
        Value::Null
    }
}

// ------------------------------------------------------------------------------- docs

#[allow(clippy::too_many_arguments)]
fn p(
    name: &str,
    ty: &str,
    unit: Option<&str>,
    required: bool,
    default: Option<&str>,
    allowed: &[&str],
    doc: &str,
) -> ParamDoc {
    ParamDoc {
        name: name.into(),
        ty: ty.into(),
        unit: unit.map(Into::into),
        required,
        default: default.map(Into::into),
        allowed: allowed.iter().map(|a| a.to_string()).collect(),
        doc: doc.into(),
    }
}

fn host_doc(
    id: &str,
    label: &str,
    summary: &str,
    params: Vec<ParamDoc>,
    result: &str,
    example: Value,
    undoable: bool,
) -> CommandDoc {
    CommandDoc {
        id: id.into(),
        label: label.into(),
        summary: summary.into(),
        params,
        open: false,
        result: Some(result.into()),
        example: (!example.is_null()).then_some(example),
        undoable,
    }
}

/// The host's own commands, documented exactly.
pub fn host_docs() -> Vec<CommandDoc> {
    let time = "{year?, month?, day?, hour?}: local time at the site";
    let camera = "{position: [x, y, z] or a reference, target: [x, y, z] or a reference (a shape id, a planting such as g4 or g4#12, `plot`, a name such as \"Pond\": its centre), fov_y_deg? = 50}";
    vec![
        host_doc(
            "agent.log",
            "Agent log",
            "What agents did in this session, newest first (at most limit, 1 to 500 of the last 500): entries {client, timeMs, command, label, ok, message, created, changed, intent}. Session state, not saved; reading commands (agent.log, document.inspect, edit.state) are not in it.",
            vec![p(
                "limit",
                "integer",
                None,
                false,
                Some("50"),
                &[],
                "At most this many entries, 1 to 500",
            )],
            "{total, entries: [{client, timeMs, command, label, ok, message, created, changed, intent}]}",
            json!({"limit": 5}),
            false,
        ),
        host_doc(
            "file.new",
            "New Project",
            "Starts an empty project (the built-in materials, the default site in Brussels, 21 June 14:00) with a new history; the open file is forgotten.",
            vec![],
            "{models: 0}",
            json!({}),
            false,
        ),
        host_doc(
            "file.open",
            "Open",
            "Opens a .visioncraft project and loads the models it refers to (paths relative to the project file); a new history starts. Models that cannot be read are listed in missingModels, the project still opens.",
            vec![p(
                "path",
                "string",
                None,
                true,
                None,
                &[],
                "A .visioncraft file beneath the read root",
            )],
            "{path, models, shapes, generators, lights, viewpoints, films, missingModels}",
            json!({"path": "garden.visioncraft"}),
            false,
        ),
        host_doc(
            "file.save",
            "Save",
            "Writes the project as .visioncraft JSON. Model paths are rewritten relative to the new folder when it changes.",
            vec![
                p(
                    "path",
                    "string",
                    None,
                    false,
                    Some("the open file's path"),
                    &[],
                    "Where to write, beneath the write root",
                ),
                p(
                    "overwrite",
                    "boolean",
                    None,
                    false,
                    Some("false"),
                    &[],
                    "Replace a file that is not the open project's own",
                ),
            ],
            "{path, bytes}",
            json!({"path": "garden.visioncraft"}),
            false,
        ),
        host_doc(
            "edit.undo",
            "Undo",
            "Takes back the last change (one command, or one whole batch).",
            vec![],
            "{can_undo, can_redo}",
            json!({}),
            false,
        ),
        host_doc(
            "edit.redo",
            "Redo",
            "Does the change last undone again.",
            vec![],
            "{can_undo, can_redo}",
            json!({}),
            false,
        ),
        host_doc(
            "edit.state",
            "Edit state",
            "Whether there is something to undo or redo, whether the project has unsaved changes (it differs from the project as last opened, created or saved: undoing back to that point is clean again) and the session's revision, which grows with every change, undo and redo. Does not change anything.",
            vec![],
            "{can_undo, can_redo, dirty, revision}",
            json!({}),
            false,
        ),
        host_doc(
            "document.inspect",
            "Inspect",
            "The project as JSON: site, time and sun, view, layers, models (bounding box, face names), shapes (areas, face names), materials, assignments, planting generators with their evaluated plant counts, species, lights, viewpoints, terrain, history.",
            vec![
                p(
                    "faces",
                    "boolean",
                    None,
                    false,
                    Some("true"),
                    &[],
                    "List the face names of every model and shape",
                ),
                p(
                    "measures",
                    "boolean",
                    None,
                    false,
                    Some("true"),
                    &[],
                    "Evaluate generators (count, truncated) and measure shapes (area_m2, footprint_m2, length_m)",
                ),
                p(
                    "placements",
                    "boolean",
                    None,
                    false,
                    Some("false"),
                    &[],
                    "Every plant's asset, position, rotation and scale (large)",
                ),
                p(
                    "full",
                    "boolean",
                    None,
                    false,
                    Some("false"),
                    &[],
                    "Materials, generators and species with all their fields, as the commands take them",
                ),
            ],
            "{path, dirty, revision, site, time, sun, view, style, layers, models, shapes, materials, assignments, generators, species, lights, viewpoints, films, terrain, history}",
            json!({"faces": false}),
            false,
        ),
        host_doc(
            "query.raycast",
            "Ray Cast",
            "The nearest surface a ray hits: models, shapes and terrain on visible layers, and with `plants` the crowns (ellipsoids) and trunks (cylinders) of the plantings as the growth model sizes them. Not a hit is {hit: false}.",
            vec![
                p(
                    "origin",
                    "[x, y, z]",
                    Some("m"),
                    true,
                    None,
                    &[],
                    "Where the ray starts",
                ),
                p(
                    "direction",
                    "[x, y, z]",
                    None,
                    true,
                    None,
                    &[],
                    "Which way it goes; any length but zero",
                ),
                p(
                    "plants",
                    "boolean",
                    None,
                    false,
                    Some("false"),
                    &[],
                    "Also hit plants: each plant of the visible plantings has a crown (an ellipsoid; trees, conifers, shrubs and hedges) and a trunk (a cylinder; trees and conifers)",
                ),
                p(
                    "years",
                    "number",
                    Some("years"),
                    false,
                    Some("the view's years, else full grown"),
                    &[],
                    "With `plants`: the years after planting the plants are sized at",
                ),
            ],
            "{hit, owner, kind: model|shape|terrain|plant, face, body, point, normal, distance; a plant: owner `g4#12` (planting and plant), part crown|trunk, planting, name, asset}",
            json!({"origin": [0, 20, 0], "direction": [0, -1, 0]}),
            false,
        ),
        host_doc(
            "query.clearance",
            "Clearance",
            "Whether one can walk and see from one place to another: the straight line on the ground between `from` and `to` against what a person cannot walk through (houses and blocks, walls and fences, water, hedges and shrubs, trunks; as plants are at `years`), and an eye-high line of sight against the same and the crowns. When something is in the way the answer plans a route round it. `from` and `to` are [x, z], [x, y, z] or a reference (a shape id, a planting, a name: its centre).",
            vec![
                p(
                    "from",
                    "[x, z] or a name",
                    Some("m"),
                    true,
                    None,
                    &[],
                    "Where it starts: [x, z], [x, y, z] or a reference",
                ),
                p(
                    "to",
                    "[x, z] or a name",
                    Some("m"),
                    true,
                    None,
                    &[],
                    "Where it ends",
                ),
                p(
                    "clearance_m",
                    "number",
                    Some("m"),
                    false,
                    Some("0.4"),
                    &[],
                    "How near an obstacle counts as in the way (half the width of a person)",
                ),
                p(
                    "eye_height_m",
                    "number",
                    Some("m"),
                    false,
                    Some("1.6"),
                    &[],
                    "Eye height for the line of sight",
                ),
                p(
                    "years",
                    "number",
                    Some("years"),
                    false,
                    Some("the view's years, else full grown"),
                    &[],
                    "The years after planting the plants are sized at",
                ),
            ],
            "{from, to, length_m, clearance_m, clear, crossings: [{what, at, distance_m}], nearest: {what, distance_m, at}, route?: {path, length_m, warnings}, sight: {clear, blocked_by?, kind?, at?}}",
            json!({"from": [5, 22.5], "to": [16, 22.5]}),
            false,
        ),
        host_doc(
            "query.sun_hours",
            "Sun Hours",
            "Hours of direct sun on one day, for a spot (`point`) or as a grid over an area (`outline`, cell centres inside it only and not inside a water area (a pond), at most 10,000 cells; the sun-map of an area shape also leaves out the areas on top of it). The sun is sampled from sunrise to sunset every `step_minutes` at the project's site and north, and a spot counts a moment as sunny when nothing between it and the sun casts shadow: models, shapes and terrain on visible layers (including context layers, the neighbours' houses), and with `plants` the crowns of woody plants (trees, palms, conifers, shrubs and hedges as ellipsoids from the growth table, `years` after planting). Evergreens and summer crowns are opaque; a deciduous crown without leaves lets 60 % of the sun through, so it counts 0.6 in winter. Clouds are not counted. Area results classify cells by the usual plant-label thresholds: full sun 6+ hours, part shade 3 to 6, shade under 3; the histogram classes are 0-2, 2-4, 4-6 and 6+ hours. Give exactly one of `point` and `outline`.",
            vec![
                p(
                    "point",
                    "[x, z]",
                    Some("m"),
                    false,
                    None,
                    &[],
                    "The spot; its height is the ground there plus `height_m`",
                ),
                p(
                    "outline",
                    "[[x, z], ...]",
                    Some("m"),
                    false,
                    None,
                    &[],
                    "The area, at least 3 points; sampled on a grid of `cell_m`",
                ),
                p(
                    "cell_m",
                    "number",
                    Some("m"),
                    false,
                    Some("1"),
                    &[],
                    "Grid cell of an outline, 0.01..1000; refused when it makes more than 10,000 cells",
                ),
                p(
                    "height_m",
                    "number",
                    Some("m"),
                    false,
                    Some("0.05"),
                    &[],
                    "How far above the ground to measure, 0..100",
                ),
                p(
                    "date",
                    "{month, day}",
                    None,
                    false,
                    Some("the project's date"),
                    &[],
                    "The day, in the project's year",
                ),
                p(
                    "season",
                    "string",
                    None,
                    false,
                    None,
                    &["spring", "summer", "autumn", "winter"],
                    "Instead of `date`: 21 March, 21 June, 21 September or 21 December (six months on south of the equator)",
                ),
                p(
                    "step_minutes",
                    "number",
                    Some("min"),
                    false,
                    Some("10"),
                    &[],
                    "Sampling step, 1..60",
                ),
                p(
                    "plants",
                    "boolean",
                    None,
                    false,
                    Some("true"),
                    &[],
                    "Plant crowns cast shadow",
                ),
                p(
                    "years",
                    "number",
                    Some("years"),
                    false,
                    Some("the view's years, else 0"),
                    &[],
                    "Years after planting the plants have grown, 0..60",
                ),
            ],
            "point: {point, height, date: {month, day}, sunrise_hour, sunset_hour, daylight_hours, sun_hours, intervals: [[start_hour, end_hour], ...], crowns, years}; outline: {date, cell_m, origin: [x0, z0], cols, rows, hours (row-major from the lowest z, the north side, null outside the outline), min, max, mean, daylight_hours, histogram: {\"0-2\", \"2-4\", \"4-6\", \"6+\"}, full_sun_share, part_shade_share, shade_share, cells, crowns, years}. Hours are local clock hours.",
            json!({"outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "season": "summer", "cell_m": 1}),
            false,
        ),
        host_doc(
            "query.sky_view",
            "Sky View",
            "How open a spot is: the share of the sky it sees, cosine-weighted as light falls on a surface (the sky view factor, 0..1; 1 under the open sky). Traced with 4096 rays against what the sky occlusion of the window is baked from: models, shapes (not areas), terrain on visible layers, and the crowns of trees, shrubs and hedges as ellipsoids of leaves that let sky through by their species' leaf density and the leaves they carry in the project's month, `years` after planting. About 0.5 at the foot of a wall, 0.15 at the bottom of a 2 m wide passage between 6 m walls, 0.3 to 0.6 under a garden tree's crown (less under a dense beech or lime than a birch). Gardeners read it with the sun hours: a spot with little sun but a high sky view is bright shade (good for ferns and hostas), a low sky view deep shade.",
            vec![
                p(
                    "point",
                    "[x, z] | [x, y, z]",
                    Some("m"),
                    true,
                    None,
                    &[],
                    "The spot: [x, z] stands on the ground there (terrain, areas, models); [x, y, z] a point in space",
                ),
                p(
                    "normal",
                    "[x, y, z]",
                    None,
                    false,
                    Some("[0, 1, 0]"),
                    &[],
                    "The way the surface faces: up for the ground, sideways for a wall",
                ),
                p(
                    "years",
                    "number",
                    Some("years"),
                    false,
                    Some("the view's years, else full grown"),
                    &[],
                    "Years after planting the crowns have grown, 0..60",
                ),
            ],
            "{point: [x, y, z], normal, sky_view, crowns, years, month}",
            json!({"point": [6, 26]}),
            false,
        ),
        host_doc(
            "query.light_check",
            "Light Check",
            "Checks every placed plant (the `flora:` species of the plantings, single placements too) against the sun its spot gets. For each plant it computes the hours of direct sun on the day (the summer solstice unless `season` or `date` say otherwise) as `query.sun_hours` does, with shadow from models, shapes, terrain and the crowns of the other plantings' woody plants; a plant is measured at half its height (up to 2 m), and the crowns of its own planting do not shade it (a planting is judged against the light from outside it; its own crown never shades the plant). The hours are held against the species' estimated light range (full sun 6+ h, part shade 3 to 6 h, full shade under 3 h; the lawn wants 4+ h; a species outside the table gets a guess from its grower type, flagged `from_table: false`; none of it is checked against open data yet). A plant is `ok` when its hours lie inside the range give or take half an hour, `too_sunny` above it, `too_shady` below it. The answer starts with the count by verdict and lists the misfits, each with 2 or 3 better-fitting species of the same grower type to swap in (closest in mature height first). Lawn tufts are many and alike: at most 25 per lawn are measured. More than 4,000 plants are refused: narrow it with `ids` or `layer`.",
            vec![
                p(
                    "season",
                    "string",
                    None,
                    false,
                    Some("summer"),
                    &["spring", "summer", "autumn", "winter"],
                    "Which day: 21 March, 21 June, 21 September or 21 December (six months on south of the equator)",
                ),
                p(
                    "date",
                    "{month, day}",
                    None,
                    false,
                    None,
                    &[],
                    "Instead of `season`: a day in the project's year",
                ),
                p(
                    "years",
                    "number",
                    Some("years"),
                    false,
                    Some("the view's years, else 0"),
                    &[],
                    "Years after planting the plants have grown, 0..60",
                ),
                p(
                    "plants",
                    "boolean",
                    None,
                    false,
                    Some("true"),
                    &[],
                    "Plant crowns cast shadow",
                ),
                p(
                    "step_minutes",
                    "number",
                    Some("min"),
                    false,
                    Some("15"),
                    &[],
                    "Sampling step of the day, 1..60",
                ),
                p(
                    "ids",
                    "[string]",
                    None,
                    false,
                    Some("all plantings"),
                    &[],
                    "Only the plants of these plantings (`g1`...)",
                ),
                p(
                    "layer",
                    "string",
                    None,
                    false,
                    Some("all layers"),
                    &[],
                    "Only the plants of the plantings on this layer",
                ),
                p(
                    "suggestions",
                    "integer",
                    None,
                    false,
                    Some("3"),
                    &[],
                    "Better-fitting species listed for each misfit, 0..5",
                ),
                p(
                    "include_ok",
                    "boolean",
                    None,
                    false,
                    Some("false"),
                    &[],
                    "Also list the plants that are fine",
                ),
                p(
                    "limit",
                    "integer",
                    None,
                    false,
                    Some("200"),
                    &[],
                    "At most this many plants are listed, 1..5000 (the counts cover all)",
                ),
            ],
            "{date, years, daylight_hours, crowns, summary: {checked, ok, too_sunny, too_shady, lawn_tufts_not_measured}, mismatches, listed, truncated, plants: [{id (`g1#12`: planting and placement index), planting, species, name, position: [x, z], hours, light: {min_hours, max_hours}, from_table, estimated, verdict (ok|too_sunny|too_shady), suggestions: [{species, name, fit, light}]}], by_planting: [{id, name, ok, too_sunny, too_shady}], by_species: [{species, name, count, ok, too_sunny, too_shady, mean_hours, light}], note}",
            json!({"season": "summer"}),
            false,
        ),
        host_doc(
            "query.ground_length",
            "Ground Length",
            "The length of a walk along path as it runs over the ground (terrain, areas and models), sampled every 0.25 m; where there is no ground it counts as height 0.",
            vec![p(
                "path",
                "[[x, z], ...]",
                Some("m"),
                true,
                None,
                &[],
                "The walk, at least 2 points",
            )],
            "{ground_length_m}",
            json!({"path": [[0, 0], [10, 0]]}),
            false,
        ),
        host_doc(
            "query.ground_height",
            "Ground Height",
            "The height plants stand on at (x, z): terrain, areas and models, the highest.",
            vec![
                p("x", "number", Some("m"), true, None, &[], "East"),
                p(
                    "z",
                    "number",
                    Some("m"),
                    true,
                    None,
                    &[],
                    "South (-z is north)",
                ),
            ],
            "{x, z, height: number or null}",
            json!({"x": 0, "z": 0}),
            false,
        ),
        host_doc(
            "catalog.search",
            "Search Catalogue",
            "Assets of the built-in catalogue whose name or a tag contains query, by name.",
            vec![
                p(
                    "query",
                    "string",
                    None,
                    false,
                    Some("\"\" (all)"),
                    &[],
                    "Part of a name or tag, any case",
                ),
                p(
                    "category",
                    "string",
                    None,
                    false,
                    None,
                    &[
                        "tree",
                        "shrub",
                        "perennial",
                        "grass",
                        "flower",
                        "hedge",
                        "furniture",
                        "pot",
                        "lighting",
                        "fence",
                        "play",
                        "vehicle",
                        "person",
                        "other",
                    ],
                    "Only this category",
                ),
                p(
                    "exposure",
                    "string",
                    None,
                    false,
                    None,
                    &["sun", "half_shade", "shade"],
                    "Only plants for this exposure",
                ),
                p(
                    "max_height_m",
                    "number",
                    Some("m"),
                    false,
                    None,
                    &[],
                    "Only plants at most this tall when mature",
                ),
                p(
                    "limit",
                    "integer",
                    None,
                    false,
                    Some("50"),
                    &[],
                    "At most this many, 1 to 500",
                ),
            ],
            "{total, assets: [{id, name, category, size_m, license, author}]}",
            json!({"query": "bench"}),
            false,
        ),
        host_doc(
            "species.list",
            "List Species",
            "The procedural plant species: the built-in ones (asset id flora:<id>) and the project's custom ones (their own ids).",
            vec![
                p(
                    "filter",
                    "string",
                    None,
                    false,
                    None,
                    &[],
                    "Part of the id, common or botanical name, any case",
                ),
                p(
                    "category",
                    "string",
                    None,
                    false,
                    None,
                    &[
                        "tree",
                        "conifer",
                        "palm",
                        "shrub",
                        "hedge",
                        "perennial",
                        "fern",
                        "grass",
                        "ground_cover",
                        "climber",
                    ],
                    "Only this category",
                ),
            ],
            "{species: [{asset, common_name, botanical_name, category, mature_height_m, mature_width_m, evergreen, bloom_months, exposure}], custom: [{asset, name, base}]}",
            json!({"category": "hedge"}),
            false,
        ),
        host_doc(
            "planting.list_presets",
            "List Planting Presets",
            "The layered plantings planting.apply_preset takes.",
            vec![],
            "{presets: [{id, name, layers}]}",
            json!({}),
            false,
        ),
        host_doc(
            "film.preview",
            "Preview Film",
            "Reads a film without drawing it: its length, frame count and where each shot starts, and with `t` the camera at that time (eye, target, field of view), the shot it is in, how far through it, what changes for that picture (time of day, date or season, years, snow, wetness, rain, haze, mist, gustiness, shafts, weather, clouds) and the sun then. The walk follows the real ground of the project (terrain, areas, models), so this is how to check the shots before film.render.",
            vec![
                p("id", "string", None, true, None, &[], "Film id"),
                p(
                    "t",
                    "number",
                    Some("s"),
                    false,
                    None,
                    &[],
                    "Film time in seconds, 0 to the duration (later is the last frame); without it only the shot list comes back",
                ),
                p(
                    "sheet",
                    "boolean",
                    None,
                    false,
                    Some("false"),
                    &[],
                    "Also draw a contact sheet: the film at evenly spaced times, small, tiled on one picture (over MCP an image block, so an agent can see the film; the answer's `sheet` has the times and the layout). Needs the renderer (a GPU)",
                ),
                p(
                    "frames",
                    "integer",
                    None,
                    false,
                    Some("9"),
                    &[],
                    "With `sheet`: how many pictures on it, 1 to 16",
                ),
                p(
                    "width",
                    "integer",
                    Some("px"),
                    false,
                    Some("320"),
                    &[],
                    "With `sheet`: the width of one picture, 64 to 640 (16:9)",
                ),
                p(
                    "quality",
                    "string",
                    None,
                    false,
                    Some("\"low\""),
                    &["low", "medium", "high", "ultra", "photo"],
                    "With `sheet`: the renderer's quality preset, as view.render",
                ),
            ],
            "{id, name, fps, duration_s, frame_count, shots: [{index, kind, start_s, duration_s}], t?, shot?, progress?, camera?: {position, target, fov_y_deg}, overrides?: {time_of_day?, date?, season?, years?, snow?, wetness?, rain?, haze?, mist?, gustiness?, shafts?, weather?, weather_to?, weather_mix?, clouds?, lightning?, lights?, white_balance?, grade?, grade_to?, grade_mix?, exposure_ev?}, time?, sun?, with `sheet`: mime, base64, sheet: {columns, frames, cell_px, size_px, times_s, shots, order}}",
            json!({"id": "f1", "t": 2.5}),
            false,
        ),
        host_doc(
            "film.render",
            "Render Film",
            "Draws a film as numbered PNGs `frame_00000.png`... in a folder beneath the write root, one view.render per frame: the film's camera, the animation clock set to the film time (so wind and water move) and the shot's time of day, date or season, years, snow, wetness, rain, haze, mist, gustiness, shafts, weather, clouds, lamp light, white balance, grade and exposure compensation for that picture only; the project does not change. Frames are counted from 0 in what is drawn, so `every` 5 gives a film of fps / 5. At most 10000 frames per call: render a long film in parts (`from`, `to`). A video needs a program such as ffmpeg: the result's `ffmpeg` is the command to run on the frames.",
            vec![
                p("id", "string", None, true, None, &[], "Film id"),
                p(
                    "dir",
                    "string",
                    None,
                    true,
                    None,
                    &[],
                    "A folder beneath the write root; made when missing",
                ),
                p(
                    "width",
                    "integer",
                    Some("px"),
                    false,
                    Some("960"),
                    &[],
                    "16 to 4096",
                ),
                p(
                    "height",
                    "integer",
                    Some("px"),
                    false,
                    Some("540"),
                    &[],
                    "16 to 4096",
                ),
                p(
                    "quality",
                    "string",
                    None,
                    false,
                    Some("\"high\""),
                    &["low", "medium", "high", "ultra", "photo"],
                    "The renderer's quality preset, as view.render (photo averages samples)",
                ),
                p(
                    "from",
                    "number",
                    Some("s"),
                    false,
                    Some("0"),
                    &[],
                    "First film time drawn",
                ),
                p(
                    "to",
                    "number",
                    Some("s"),
                    false,
                    Some("the end"),
                    &[],
                    "Last film time drawn",
                ),
                p(
                    "every",
                    "integer",
                    None,
                    false,
                    Some("1"),
                    &[],
                    "Draw every Nth frame, 1 to 10000",
                ),
                p(
                    "samples",
                    "integer",
                    None,
                    false,
                    Some("8 at quality photo, else 1"),
                    &[],
                    "Frames averaged per film frame, 1 to 1024 (as view.render's)",
                ),
                p(
                    "aperture",
                    "number",
                    None,
                    false,
                    Some("as view.render"),
                    &[],
                    "Lens f-number for every frame, 1 to 32; null all sharp",
                ),
                p(
                    "focus_m",
                    "any",
                    Some("m"),
                    false,
                    Some("as view.render"),
                    &[],
                    "Focus for every frame: metres, \"auto\" (refocused each frame) or \"target\"",
                ),
                p(
                    "focus_point",
                    "any",
                    None,
                    false,
                    Some("[0.5, 0.5]"),
                    &[],
                    "Where \"auto\" focus looks, [x, y] from 0 to 1",
                ),
                p(
                    "shutter_s",
                    "number",
                    Some("s"),
                    false,
                    Some("0"),
                    &[],
                    "Shutter time per frame, 0 to 1 (1 / (2 fps) is the film look's 180 degree shutter)",
                ),
                p(
                    "vignette",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "A light vignette 0..1",
                ),
                p(
                    "grain",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "Film grain 0..1",
                ),
                p(
                    "preview",
                    "string",
                    None,
                    false,
                    Some("not set"),
                    &["sheet"],
                    "\"sheet\": also return a contact sheet of up to nine of the frames drawn (over MCP an image block), so a film can be seen without any other tool",
                ),
            ],
            "{dir, frames, framePattern, fps, width, height, duration_s, first_t, last_t, renderer, ffmpeg, with preview: mime, base64, sheet}",
            json!({"id": "f1", "dir": "frames", "width": 640, "height": 360, "every": 5}),
            false,
        ),
        host_doc(
            "view.render",
            "Render",
            "Draws the project with the app's renderer, without a window: PNG to path beneath the write root, else returned as base64. Plants are grown before the picture is taken. The project does not change.",
            vec![
                p(
                    "path",
                    "string",
                    None,
                    false,
                    None,
                    &[],
                    "A .png beneath the write root; without it the PNG comes back as {mime, base64}",
                ),
                p(
                    "width",
                    "integer",
                    Some("px"),
                    false,
                    Some("800"),
                    &[],
                    "16 to 4096",
                ),
                p(
                    "height",
                    "integer",
                    Some("px"),
                    false,
                    Some("600"),
                    &[],
                    "16 to 4096",
                ),
                p(
                    "viewpoint",
                    "string",
                    None,
                    false,
                    None,
                    &[],
                    "A viewpoint's id or name: its camera, and its time when it has one",
                ),
                p(
                    "camera",
                    "object",
                    None,
                    false,
                    Some("the whole scene, framed"),
                    &[],
                    camera,
                ),
                p(
                    "time",
                    "object",
                    None,
                    false,
                    Some("the viewpoint's, else the project's"),
                    &[],
                    time,
                ),
                p(
                    "quality",
                    "string",
                    None,
                    false,
                    Some("\"high\""),
                    &["low", "medium", "high", "ultra", "photo"],
                    "The renderer's quality preset; photo averages `samples` frames at ULTRA (soft shadows, smooth edges and occlusion, optional depth of field), taking every lens setting it is not given from view.set_photo",
                ),
                p(
                    "shadow_filter",
                    "string",
                    None,
                    false,
                    Some("the preset's"),
                    &["hard", "pcf4", "pcf9", "pcf16"],
                    "The sun's shadow filter for this picture, over the preset's: hard is one comparison tap (the sharpest edge); pcf4 and pcf9 are fixed 2x2 and 3x3 grids a few shadow texels wide; pcf16 spreads 16 taps (a disk) over `shadow_radius_m` metres in the world, the same penumbra in every cascade. At quality low and medium only the 1- and 4-tap filters are compiled: pcf9 and pcf16 run as pcf4 there",
                ),
                p(
                    "shadow_radius_m",
                    "number",
                    Some("m"),
                    false,
                    Some("0.03"),
                    &[],
                    "The penumbra's radius in the world for this picture, 0 to 1, over the preset's: only the 16-tap disk spreads over it, so it needs `shadow_filter` pcf16 or none (with hard, pcf4 or pcf9 it is an error); alone it changes a preset that filters with 16 taps (ultra, photo). Beyond 6 shadow texels of a cascade the penumbra stops growing there",
                ),
                p(
                    "samples",
                    "integer",
                    None,
                    false,
                    Some("quality photo: view.set_photo's (64); else 1"),
                    &[],
                    "Frames averaged, 1 to 1024, at any quality: each moved by a fraction of a pixel, the sun's light within its disc and the noise of occlusion, shafts and leaf edges by the frame, and their mean (in linear HDR) goes through bloom and the tone map once. 1 without a lens or shutter is the plain still",
                ),
                p(
                    "aperture",
                    "number",
                    None,
                    false,
                    Some("quality photo: view.set_photo's; else none (all sharp)"),
                    &[],
                    "The lens' f-number for depth of field, 1 to 32 (a full-frame lens for the field of view; f/2.8 blurs what lies behind a subject a few metres off); null all sharp",
                ),
                p(
                    "aperture_m",
                    "number",
                    Some("m"),
                    false,
                    Some("0 (all sharp)"),
                    &[],
                    "Instead of `aperture`: the lens' radius, 0 to 0.5",
                ),
                p(
                    "focus_m",
                    "any",
                    Some("m"),
                    false,
                    Some("quality photo: view.set_photo's; else the camera target"),
                    &[],
                    "With a lens: the distance in focus, 0.1 to 1000, \"auto\" (the depth the picture shows at `focus_point`) or \"target\" (the camera target's distance)",
                ),
                p(
                    "focus_point",
                    "any",
                    None,
                    false,
                    Some("[0.5, 0.5], the centre"),
                    &[],
                    "Where `focus_m` \"auto\" looks: [x, y] from 0 to 1, from the picture's top left",
                ),
                p(
                    "shutter_s",
                    "number",
                    Some("s"),
                    false,
                    Some("0"),
                    &[],
                    "Shutter time 0 to 1 from `animation_s`: plants in the wind, water and rain blur with their motion (give samples enough to cover it: 32 or more)",
                ),
                p(
                    "vignette",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "A light vignette 0..1 for this picture only (0.3 about half a stop off the corners)",
                ),
                p(
                    "grain",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "Film grain 0..1 for this picture only, after the tone map (0.2 fine)",
                ),
                p(
                    "animation_s",
                    "number",
                    Some("s"),
                    false,
                    Some("0"),
                    &[],
                    "The animation clock the picture is taken at, 0 to 86400: where the wind has the plants swaying and the water moving (view.set wind_speed_mps). Two pictures at times a fraction of a second apart show the motion",
                ),
                p(
                    "years",
                    "number",
                    None,
                    false,
                    Some("the viewpoint's, else the project's"),
                    &[],
                    "Years after planting to show the plants at (0..60), for this picture only",
                ),
                p(
                    "season",
                    "string",
                    None,
                    false,
                    Some("the project's (or the viewpoint's)"),
                    &["spring", "summer", "autumn", "winter"],
                    "The season for this picture only: its day at the site, the hour kept",
                ),
                p(
                    "snow",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "Snow 0..1 for this picture only; on its own it brings winter, with another season it is refused",
                ),
                p(
                    "wetness",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "Wet surfaces 0..1 for this picture only (darker, glossier, puddles above about 0.5)",
                ),
                p(
                    "rain",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "Rain 0..1 for this picture only: streaks, ripples, overcast, fog, wet surfaces",
                ),
                p(
                    "haze",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "Haze as a turbidity for this picture only (0 to 50; 2.5 a clear day, 20 misty)",
                ),
                p(
                    "mist",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "Mist 0..1 for this picture only (visibility 1.3 km at 0.3, 330 m at 0.6, 120 m at 1)",
                ),
                p(
                    "gustiness",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "Gusts 0..1 for this picture only: patches of wind rolling over lawns and through crowns (set `animation_s` to move them)",
                ),
                p(
                    "shafts",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "Light shafts 0..1 for this picture only: beams of sun through the mist, rain fog or haze where crowns and houses shadow it",
                ),
                p(
                    "weather",
                    "string",
                    None,
                    false,
                    Some("the project's"),
                    &["clear", "fair", "cloudy", "overcast", "rain", "storm"],
                    "A weather for this picture only (as view.set `weather`): the clouds, rain, wetness, mist, gusts and wind of that preset; rain, mist, wetness, gustiness and clouds given as well win",
                ),
                p(
                    "weather_to",
                    "string",
                    None,
                    false,
                    Some("not set"),
                    &["clear", "fair", "cloudy", "overcast", "rain", "storm"],
                    "A second weather mixed over `weather` by `weather_mix`: a sky between two weathers, as a film's `weather` [from, to] eases through them",
                ),
                p(
                    "weather_mix",
                    "number",
                    None,
                    false,
                    Some("0.5"),
                    &[],
                    "With `weather_to`: its share, 0 to 1",
                ),
                p(
                    "clouds",
                    "any",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "The clouds for this picture only: a cover 0..1, or an object merged into the project's as view.set `clouds` takes it ({kind, cover, altitude_m, speed_mps, volumetric, sun_gap, lightning})",
                ),
                p(
                    "lightning",
                    "number",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "How often lightning flashes in a storm sky for this picture only, 0 to 1 (as view.set `clouds.lightning`); set `animation_s` to choose the moment: the flashes follow the animation clock",
                ),
                p(
                    "sky_occlusion",
                    "boolean",
                    None,
                    false,
                    Some("the project's, else on"),
                    &[],
                    "Sky occlusion for this picture only: every surface lit by the sky it sees (view.set `sky_occlusion`)",
                ),
                p(
                    "white_balance",
                    "any",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "White balance for this picture only: \"daylight\", \"auto\" or a temperature in kelvin (2000 to 12000)",
                ),
                p(
                    "grade",
                    "string",
                    None,
                    false,
                    Some("the project's"),
                    &["neutral", "warm", "cool", "soft"],
                    "Colour grade for this picture only",
                ),
                p(
                    "grade_to",
                    "string",
                    None,
                    false,
                    Some("not set"),
                    &["neutral", "warm", "cool", "soft"],
                    "A second grade mixed over `grade` (or the project's) by `grade_mix`: how a film eases from one look to another",
                ),
                p(
                    "grade_mix",
                    "number",
                    None,
                    false,
                    Some("0.5"),
                    &[],
                    "With `grade_to`: its share, 0 to 1",
                ),
                p(
                    "lights",
                    "any",
                    None,
                    false,
                    Some("the project's"),
                    &[],
                    "The lamps for this picture only: true, false or the share of their light, 0 to 1 (a fade; any value forces the lamps on or off, 0 off)",
                ),
                p(
                    "exposure_offset_ev",
                    "number",
                    Some("EV"),
                    false,
                    Some("0"),
                    &[],
                    "Exposure compensation in stops relative to the automatic exposure (0 none, +1 one stop brighter), -8 to 8. Not view.set's exposure_ev, which is an absolute exposure value (0 is white in daylight)",
                ),
            ],
            "{path? | mime, base64; width, height, camera, time, years, ...the renderer's info}",
            json!({"width": 960, "height": 540, "viewpoint": "From the gate"}),
            false,
        ),
    ]
}

/// Every command, sorted by id: the host's and the core's.
pub fn docs(session: &Session) -> Vec<CommandDoc> {
    let mut all = host_docs();
    all.extend(session.command_specs().into_iter().map(from_spec));
    // The host runs this one with the geometry of the models and the renderer's building rule.
    for d in all.iter_mut().filter(|d| d.id == "query.quantities") {
        d.summary = d.summary.replace(
            visioncraft_core::quantities::CORE_NOTE,
            visioncraft_core::quantities::HOST_NOTE,
        );
    }
    all.sort_by(|a, b| a.id.cmp(&b.id));
    all
}

fn enabled(session: &Session, id: &str) -> Result<(), String> {
    match id {
        "edit.undo" if !session.can_undo() => Err("nothing to undo".into()),
        "edit.redo" if !session.can_redo() => Err("nothing to redo".into()),
        _ => Ok(()),
    }
}

/// What a listing lists.
#[derive(Default)]
pub struct ListQuery<'a> {
    /// Case-insensitive part of an id or label.
    pub filter: Option<&'a str>,
    /// One command by its exact id.
    pub id: Option<&'a str>,
    pub enabled_only: bool,
    pub brief: bool,
}

/// `{"commands": [...]}`, as `engine.commands` and `command_list` give it.
pub fn command_list(session: &Session, query: &ListQuery) -> Value {
    let needle = query.filter.map(str::to_lowercase);
    let commands: Vec<Value> = docs(session)
        .iter()
        .filter(|d| query.id.is_none_or(|id| d.id == id))
        .filter(|d| {
            needle.as_deref().is_none_or(|n| {
                d.id.to_lowercase().contains(n) || d.label.to_lowercase().contains(n)
            })
        })
        .filter_map(|d| {
            let on = enabled(session, &d.id);
            (on.is_ok() || !query.enabled_only).then(|| d.to_json(query.brief, on))
        })
        .collect();
    json!({ "commands": commands })
}

/// The command ids nearest to a misspelt one.
fn suggest(wrong: &str, docs: &[CommandDoc]) -> String {
    fn distance(a: &str, b: &str) -> usize {
        let b: Vec<char> = b.chars().collect();
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut prev = row[0];
            row[0] = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let next = (prev + usize::from(ca != *cb))
                    .min(row[j] + 1)
                    .min(row[j + 1] + 1);
                prev = row[j + 1];
                row[j + 1] = next;
            }
        }
        row[b.len()]
    }
    let best = docs
        .iter()
        .map(|d| (distance(wrong, &d.id), &d.id))
        .min()
        .filter(|(d, _)| *d <= 3);
    match best {
        Some((_, id)) => format!("unknown command `{}`; did you mean `{id}`?", shorten(wrong)),
        None => format!(
            "unknown command `{}`; engine.commands (or the MCP tool command_list) lists them",
            shorten(wrong)
        ),
    }
}

// ---------------------------------------------------------------------------- running

/// Runs one command by id. Errors start with the command's id and name the parameter.
pub fn execute(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    if !matches!(params, Value::Null | Value::Object(_)) {
        return Err(format!(
            "{id}: params must be a JSON object, got {}",
            describe(params)
        ));
    }
    let empty = Value::Object(Map::new());
    let params = if params.is_null() { &empty } else { params };
    let result = match id {
        "file.new" => file_new(host, params),
        "file.open" => file_open(host, params),
        "file.save" => file_save(host, params),
        "edit.undo" | "edit.redo" => history(host, id, params),
        "edit.state" => {
            Fields::new(id, "parameter", Some(params), &[])?;
            let (session, state) = host.parts();
            Ok(EditState::of(session, state.saved.as_ref()).to_json())
        }
        "document.inspect" => {
            let a = Fields::new(
                id,
                "parameter",
                Some(params),
                &["faces", "measures", "placements", "full"],
            )?;
            let v = Verbosity {
                faces: a.bool("faces")?.unwrap_or(true),
                measures: a.bool("measures")?.unwrap_or(true),
                placements: a.bool("placements")?.unwrap_or(false),
                full: a.bool("full")?.unwrap_or(false),
            };
            let (session, state) = host.parts();
            Ok(inspect::inspect(session, state, &v))
        }
        "query.raycast" => {
            let a = Fields::new(
                id,
                "parameter",
                Some(params),
                &["origin", "direction", "plants", "years"],
            )?;
            let origin = need::<[f32; 3]>(id, params, "origin")?;
            let dir = need::<[f32; 3]>(id, params, "direction")?;
            if dir.iter().all(|c| *c == 0.0) {
                return Err(format!("{id}: `direction` must not be [0, 0, 0]"));
            }
            // `plants`: the crowns (ellipsoids) and trunks (cylinders) of the plantings too, as
            // they are `years` after planting (the view's years; else full grown).
            let plants = if a.bool("plants")?.unwrap_or(false) {
                let years = a.number("years")?.map(|y| y as f32);
                Some(years.or(host.session().project().view.years))
            } else {
                None
            };
            let (session, state) = host.parts();
            Ok(inspect::raycast_json(session, state, origin, dir, plants))
        }
        "query.quantities" => {
            Fields::new(id, "parameter", Some(params), &["ids", "layer"])?;
            let ids: Option<Vec<String>> = take(id, params, "ids")?;
            let layer: Option<String> = take(id, params, "layer")?;
            let (session, state) = host.parts();
            inspect::quantities_json(session, state, ids.as_deref(), layer.as_deref())
                .map_err(|e| format!("{id}: {e}"))
        }
        "query.sun_hours" => {
            Fields::new(
                id,
                "parameter",
                Some(params),
                &[
                    "point",
                    "outline",
                    "cell_m",
                    "height_m",
                    "date",
                    "season",
                    "step_minutes",
                    "plants",
                    "years",
                ],
            )?;
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Date {
                month: u32,
                day: u32,
            }
            let date = take::<Date>(id, params, "date")?.map(|d| (d.month, d.day));
            let season = match take::<String>(id, params, "season")? {
                Some(s) => Some(visioncraft_core::Season::parse(&s).ok_or_else(|| {
                    format!("{id}: `season` must be spring, summer, autumn or winter, not `{s}`")
                })?),
                None => None,
            };
            let q = inspect::SunHoursQuery {
                point: take(id, params, "point")?,
                outline: take(id, params, "outline")?,
                height_m: take(id, params, "height_m")?.unwrap_or(0.05),
                date,
                season,
                step_minutes: take(id, params, "step_minutes")?.unwrap_or(10.0),
                plants: take(id, params, "plants")?.unwrap_or(true),
                years: take(id, params, "years")?,
                cell_m: take(id, params, "cell_m")?.unwrap_or(1.0),
            };
            let (session, state) = host.parts();
            inspect::sun_hours_json(session, state, &q)
        }
        "query.sky_view" => {
            Fields::new(id, "parameter", Some(params), &["point", "normal", "years"])?;
            let point: Vec<f32> =
                take(id, params, "point")?.ok_or_else(|| format!("{id}: `point` is required"))?;
            let normal: Option<[f32; 3]> = take(id, params, "normal")?;
            let years: Option<f32> = take(id, params, "years")?;
            let (session, state) = host.parts();
            inspect::sky_view_json(session, state, &point, normal, years)
        }
        "query.plants_for_light" => {
            Fields::new(
                id,
                "parameter",
                Some(params),
                &[
                    "hours", "point", "outline", "season", "date", "grower", "limit", "cell_m",
                    "plants", "years",
                ],
            )?;
            let point: Option<[f32; 2]> = take(id, params, "point")?;
            let outline: Option<Vec<[f32; 2]>> = take(id, params, "outline")?;
            let hours: Option<f32> = take(id, params, "hours")?;
            if u8::from(point.is_some()) + u8::from(outline.is_some()) + u8::from(hours.is_some())
                > 1
            {
                return Err(format!(
                    "{id}: give exactly one of `hours`, `point` and `outline`"
                ));
            }
            if point.is_none() && outline.is_none() {
                // The plain command: the hours are given.
                run_core(host, id, params)
            } else {
                let (date, season) = light_date(id, params)?;
                let grower = take::<String>(id, params, "grower")?
                    .map(|g| {
                        visioncraft_core::plant_light::parse_grower(&g).ok_or_else(|| {
                            format!("{id}: `grower` `{g}` is not one of tree, conifer, palm, shrub, clipped, grass, perennial, ground_cover, climber, lawn")
                        })
                    })
                    .transpose()?;
                let limit = take::<usize>(id, params, "limit")?.unwrap_or(25);
                if !(1..=200).contains(&limit) {
                    return Err(format!("{id}: `limit` must be 1..200"));
                }
                let q = inspect::PlantsForLightQuery {
                    point,
                    outline,
                    date,
                    season,
                    grower,
                    limit,
                    cell_m: take(id, params, "cell_m")?.unwrap_or(1.0),
                    plants: take(id, params, "plants")?.unwrap_or(true),
                    years: take(id, params, "years")?,
                };
                let (session, state) = host.parts();
                inspect::plants_for_light_json(session, state, &q)
            }
        }
        "query.light_check" => {
            Fields::new(
                id,
                "parameter",
                Some(params),
                &[
                    "season",
                    "date",
                    "years",
                    "plants",
                    "step_minutes",
                    "ids",
                    "layer",
                    "suggestions",
                    "include_ok",
                    "limit",
                ],
            )?;
            let (date, season) = light_date(id, params)?;
            let suggestions = take::<usize>(id, params, "suggestions")?.unwrap_or(3);
            let limit = take::<usize>(id, params, "limit")?.unwrap_or(200);
            if suggestions > 5 {
                return Err(format!("{id}: `suggestions` must be 0..5"));
            }
            if !(1..=5000).contains(&limit) {
                return Err(format!("{id}: `limit` must be 1..5000"));
            }
            let q = inspect::LightCheckQuery {
                date,
                season,
                years: take(id, params, "years")?,
                plants: take(id, params, "plants")?.unwrap_or(true),
                step_minutes: take(id, params, "step_minutes")?.unwrap_or(15.0),
                ids: take(id, params, "ids")?,
                layer: take(id, params, "layer")?,
                suggestions,
                include_ok: take(id, params, "include_ok")?.unwrap_or(false),
                limit,
            };
            let (session, state) = host.parts();
            inspect::light_check_json(session, state, &q)
        }
        "planting.add_border" => add_border_host(host, params),
        "query.ground_length" => {
            Fields::new(id, "parameter", Some(params), &["path"])?;
            let path = need::<Vec<[f32; 2]>>(id, params, "path")?;
            if path.len() < 2 {
                return Err(format!("{id}: `path` needs at least 2 points"));
            }
            let (session, state) = host.parts();
            Ok(inspect::ground_length_json(session, state, &path))
        }
        "query.ground_height" => {
            Fields::new(id, "parameter", Some(params), &["x", "z"])?;
            let (x, z) = (need::<f32>(id, params, "x")?, need::<f32>(id, params, "z")?);
            let (session, state) = host.parts();
            Ok(inspect::ground_json(session, state, x, z))
        }
        "catalog.search" => catalog_search(host, params),
        "species.list" => species_list(host, params),
        "planting.list_presets" => {
            Fields::new(id, "parameter", Some(params), &[])?;
            let presets: Vec<Value> = visioncraft_core::planting_presets()
                .iter()
                .map(|p| json!({"id": p.id, "name": p.name, "layers": p.layers.len()}))
                .collect();
            Ok(json!({ "presets": presets }))
        }
        "view.render" => {
            let params = resolve_camera(host, params)?;
            view_render(host, &params)
        }
        "film.add_shot" | "film.update_shot" | "film.orbit" | "film.flythrough" | "film.reveal"
        | "viewpoint.add" | "viewpoint.update" => run_resolved(host, id, params),
        "query.clearance" => {
            let a = Fields::new(
                id,
                "parameter",
                Some(params),
                &["from", "to", "clearance_m", "eye_height_m", "years"],
            )?;
            let (from, to) = (
                a.get("from").ok_or_else(|| a.missing("from"))?,
                a.get("to").ok_or_else(|| a.missing("to"))?,
            );
            let q = inspect::ClearanceQuery {
                from,
                to,
                clearance_m: a.number("clearance_m")?.map(|c| c as f32),
                eye_height_m: a.number("eye_height_m")?.map_or(1.6, |c| c as f32),
                years: a.number("years")?.map(|y| y as f32),
            };
            let (session, state) = host.parts();
            inspect::clearance_json(session, state, &q).map_err(|e| format!("{id}: {e}"))
        }
        "film.preview" => crate::film::preview(host, params),
        "film.render" => crate::film::render(host, params),
        "agent.log" => agent_log(host, params),
        _ => run_core(host, id, params),
    }?;
    Ok(result)
}

/// A director's command with its references resolved and routes planned over the host's scene
/// (model geometry, plants as drawn, the ground), then run; what was resolved, the route and
/// the warnings are in the answer.
fn run_resolved(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    let (given, notes) = {
        let (session, state) = host.parts();
        inspect::resolve_params(session, state, id, params).map_err(|e| format!("{id}: {e}"))?
    };
    let mut result = run_core(host, id, &given)?;
    if result.is_null() && (!notes.warnings.is_empty() || !notes.resolved.is_empty()) {
        result = json!({});
    }
    if let Some(map) = result.as_object_mut() {
        for key in ["resolved", "route", "warnings"] {
            map.remove(key);
        }
    }
    notes.apply(&mut result);
    Ok(result)
}

/// `view.render`'s `camera` with `position` and `target` that are references made points.
fn resolve_camera(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let named = ["position", "target"]
        .iter()
        .any(|k| params["camera"][k].is_string() || params["camera"][k].is_object());
    if !named {
        return Ok(params.clone());
    }
    let (session, state) = host.parts();
    inspect::resolve_params(session, state, "view.render", params)
        .map(|(p, _)| p)
        .map_err(|e| format!("view.render: {e}"))
}

fn take<T: serde::de::DeserializeOwned>(
    id: &str,
    params: &Value,
    key: &str,
) -> Result<Option<T>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| format!("{id}: `{key}`: {e}")),
    }
}

fn need<T: serde::de::DeserializeOwned>(id: &str, params: &Value, key: &str) -> Result<T, String> {
    take(id, params, key)?.ok_or_else(|| format!("{id}: missing `{key}`"))
}

/// A core command: unknown keys refused (where the docs list them all), then `Session::run`.
fn run_core(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    if id.starts_with("planting.") {
        check_assets(host, id, params)?;
    }
    let (session, state) = host.parts();
    if session.command_spec(id).is_none() {
        return Err(suggest(id, &docs(session)));
    }
    // A model whose file cannot be read is refused before it is added.
    let preloaded = if id == "model.add" {
        let kind: Option<ModelKind> = take(id, params, "kind")?;
        let source: Option<String> = take(id, params, "source")?;
        match (kind, source) {
            (Some(kind), Some(source)) => Some(
                state
                    .load(kind, &source)
                    .map_err(|e| format!("{id}: {e}"))?,
            ),
            _ => None,
        }
    } else {
        None
    };
    // A project image (an underlay's, a wall photo's) is read here: its size is recorded when the
    // caller gave none, and a file that is not there or not an image is refused with a message.
    let mut params = std::borrow::Cow::Borrowed(params);
    let mut image_file = None;
    if matches!(
        id,
        "underlay.add" | "underlay.set" | "wall_photo.add" | "wall_photo.set"
    ) && let Some(file) = take::<String>(id, &params, "file")?
    {
        let given: Option<[u32; 2]> = take(id, &params, "size_px")?;
        match state.load_image(&file) {
            Ok((bytes, size)) => {
                if given.is_none() {
                    params.to_mut()["size_px"] = json!(size);
                }
                image_file = Some((file, bytes));
            }
            // A caller that states the size may name a file this host does not have; it is
            // then reported as missing, as a model's source is.
            Err(why) if given.is_none() || why.contains("not a PNG or JPEG") => {
                return Err(format!("{id}: {why}"));
            }
            Err(why) => {
                state.image_missing.insert(file, why);
            }
        }
    }
    let result = session
        .run(id, params.clone().into_owned())
        .map_err(|e| format!("{id}: {e}"))?;
    if let Some((file, bytes)) = image_file {
        state.image_missing.remove(&file);
        state.images.insert(file, bytes);
    }
    if let (Some(loaded), Some(new)) = (preloaded, result["id"].as_str()) {
        state.missing.remove(new);
        state.models.insert(new.to_string(), loaded);
    }
    state.sync_models(session.project());
    Ok(result)
}

/// Every `asset` string in `v` (a single's, a palette's, a storey's).
fn asset_ids<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                match v.as_str() {
                    Some(s) if k == "asset" => out.push(s),
                    _ => asset_ids(v, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|v| asset_ids(v, out)),
        _ => {}
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = std::mem::replace(&mut row[0], i + 1);
        for (j, cb) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = (diag + usize::from(ca != *cb)).min(up + 1).min(row[j] + 1);
            diag = up;
        }
    }
    row[b.len()]
}

/// Planting commands refuse an asset id that resolves to nothing (a saved file with one still
/// loads, drawn as a placeholder): a catalogue or fixture asset, `flora:<species>`, a custom
/// species of the project or `user:<object>`.
fn check_assets(host: &mut dyn Host, id: &str, params: &Value) -> Result<(), String> {
    let mut asked = Vec::new();
    asset_ids(params, &mut asked);
    if asked.is_empty() {
        return Ok(());
    }
    let extra = host.extra_assets();
    let (session, state) = host.parts();
    let p = session.project();
    let mut valid: Vec<String> = visioncraft_flora::species()
        .iter()
        .map(|s| format!("flora:{}", s.id))
        .chain(p.species.iter().map(|c| c.id.clone()))
        .chain(p.objects.iter().map(|o| o.asset_id()))
        .chain(state.catalog.assets.iter().map(|a| a.id.clone()))
        .chain(extra)
        .collect();
    valid.sort();
    valid.dedup();
    if let Some(bad) = asked
        .into_iter()
        .find(|a| valid.binary_search_by(|v| v.as_str().cmp(a)).is_err())
    {
        let bare = |s: &str| s.rsplit(':').next().unwrap_or(s).to_string();
        let mut near: Vec<(usize, &String)> = valid
            .iter()
            .map(|v| (edit_distance(&bare(bad), &bare(v)), v))
            .collect();
        near.sort();
        let near: Vec<String> = near.iter().take(5).map(|(_, v)| format!("`{v}`")).collect();
        return Err(format!(
            "{id}: unknown asset `{bad}`; closest: {}",
            near.join(", ")
        ));
    }
    Ok(())
}

fn history(host: &mut dyn Host, id: &str, params: &Value) -> Result<Value, String> {
    Fields::new(id, "parameter", Some(params), &[])?;
    let (session, state) = host.parts();
    let done = if id == "edit.undo" {
        session.undo()
    } else {
        session.redo()
    };
    if !done {
        return Err(format!("{id}: nothing to {}", &id[5..]));
    }
    state.sync_models(session.project());
    Ok(json!({"can_undo": session.can_undo(), "can_redo": session.can_redo()}))
}

/// What `edit.state` reports: the history and whether there is unsaved work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditState {
    pub can_undo: bool,
    pub can_redo: bool,
    /// The project differs from `saved`, the project as last opened, created or saved.
    pub dirty: bool,
    /// The session's revision: grows with every change, undo and redo.
    pub revision: u64,
}

impl EditState {
    pub fn of(session: &Session, saved: Option<&Project>) -> Self {
        Self {
            can_undo: session.can_undo(),
            can_redo: session.can_redo(),
            dirty: saved != Some(session.project()),
            revision: session.revision(),
        }
    }

    pub fn to_json(self) -> Value {
        json!({"can_undo": self.can_undo, "can_redo": self.can_redo, "dirty": self.dirty, "revision": self.revision})
    }
}

fn file_new(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    Fields::new("file.new", "parameter", Some(params), &[])?;
    let (session, state) = host.parts();
    *session = Session::new(Project::default());
    state.file = None;
    state.models.clear();
    state.missing.clear();
    state.saved = Some(Project::default());
    Ok(json!({"models": 0}))
}

fn file_open(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "file.open";
    let a = Fields::new(id, "parameter", Some(params), &["path"])?;
    let given = a.required_str("path")?;
    let (session, state) = host.parts();
    let path = state
        .access
        .resolve(given, false, None)
        .map_err(|e| format!("{id}: {e}"))?;
    let bytes =
        std::fs::read(&path).map_err(|e| format!("{id}: cannot read `{}`: {e}", path.display()))?;
    let project = Project::from_json_bytes(&bytes).map_err(|e| format!("{id}: `{given}`: {e}"))?;
    *session = Session::new(project.clone());
    state.file = Some(std::path::absolute(&path).unwrap_or(path));
    state.models.clear();
    state.missing.clear();
    state.saved = Some(project.clone());
    state.sync_models(&project);
    Ok(json!({
        "path": state.file.as_ref().map(|f| f.display().to_string()),
        "models": project.models.len(), "shapes": project.shapes.len(),
        "generators": project.generators.len(), "lights": project.lights.len(),
        "viewpoints": project.viewpoints.len(), "films": project.films.len(),
        "missingModels": missing_json(state, &project),
    }))
}

/// `target` relative to `folder` when it lies beneath it, else absolute.
fn source_from(folder: &Path, target: &Path) -> String {
    let target = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    let folder = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
    target
        .strip_prefix(&folder)
        .unwrap_or(&target)
        .to_string_lossy()
        .replace('\\', "/")
}

fn file_save(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "file.save";
    let a = Fields::new(id, "parameter", Some(params), &["path", "overwrite"])?;
    let overwrite = a.bool("overwrite")?.unwrap_or(false);
    let (session, state) = host.parts();
    let target: PathBuf = match a.str("path")? {
        Some(p) => state
            .access
            .resolve(p, true, None)
            .map_err(|e| format!("{id}: {e}"))?,
        None => {
            let own = state
                .file
                .clone()
                .ok_or_else(|| format!("{id}: the project has no file yet; give `path`"))?;
            state
                .access
                .resolve(&own.to_string_lossy(), true, None)
                .map_err(|e| format!("{id}: {e}"))?
        }
    };
    let target = std::path::absolute(&target).unwrap_or(target);
    if target.exists() && state.file.as_ref() != Some(&target) && !overwrite {
        return Err(format!(
            "{id}: `{}` exists and is not this project's file; give overwrite: true to replace it",
            target.display()
        ));
    }
    let folder = target.parent().map(Path::to_path_buf).unwrap_or_default();
    // Model paths follow the file to its new folder.
    let mut project = session.project().clone();
    let mut moved = false;
    for m in &mut project.models {
        // An example's model travels inside the program: its source is a name, not a path.
        if visioncraft_core::example_file(&m.source).is_some() {
            continue;
        }
        if let Some(l) = state.models.get_mut(&m.id) {
            let source = source_from(&folder, &l.path);
            if source != m.source {
                m.source = source.clone();
                l.source = source;
                moved = true;
            }
        }
    }
    let bytes = project.to_json_bytes();
    std::fs::write(&target, &bytes)
        .map_err(|e| format!("{id}: cannot write `{}`: {e}", target.display()))?;
    if moved {
        session.replace_project(project.clone());
    }
    state.file = Some(target.clone());
    state.saved = Some(project);
    Ok(json!({"path": target.display().to_string(), "bytes": bytes.len()}))
}

fn catalog_search(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "catalog.search";
    let a = Fields::new(
        id,
        "parameter",
        Some(params),
        &["query", "category", "exposure", "max_height_m", "limit"],
    )?;
    let category = take(id, params, "category")?;
    let exposure = take(id, params, "exposure")?;
    let max_h = a.number("max_height_m")?.map(|h| h as f32);
    let limit = a.integer("limit", 1, 500)?.unwrap_or(50) as usize;
    let state = host.state();
    let found = state
        .catalog
        .search(a.str("query")?.unwrap_or(""), category, exposure, max_h);
    let assets: Vec<Value> = found
        .iter()
        .take(limit)
        .map(|x| {
            json!({"id": x.id, "name": x.name, "category": x.category, "size_m": x.size_m,
                   "license": x.license, "author": x.author, "plant": x.plant})
        })
        .collect();
    Ok(json!({"total": found.len(), "assets": assets}))
}

fn snake(debug: impl std::fmt::Debug) -> String {
    let mut out = String::new();
    for (i, c) in format!("{debug:?}").chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn species_list(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "species.list";
    let a = Fields::new(id, "parameter", Some(params), &["filter", "category"])?;
    let filter = a.str("filter")?.map(str::to_lowercase);
    let category = a.str("category")?;
    let matches = |texts: &[&str]| {
        filter
            .as_deref()
            .is_none_or(|f| texts.iter().any(|t| t.to_lowercase().contains(f)))
    };
    let species: Vec<Value> = visioncraft_flora::species()
        .iter()
        .filter(|s| category.is_none_or(|c| snake(s.category) == c))
        .filter(|s| matches(&[s.id, s.common_name, s.botanical_name]))
        .map(|s| {
            json!({"asset": format!("flora:{}", s.id), "common_name": s.common_name,
                   "botanical_name": s.botanical_name, "category": snake(s.category),
                   "mature_height_m": s.mature_height_m, "mature_width_m": s.mature_width_m,
                   "evergreen": s.evergreen, "bloom_months": s.bloom_months, "exposure": snake(s.exposure)})
        })
        .collect();
    let custom: Vec<Value> = host
        .session()
        .project()
        .species
        .iter()
        .filter(|s| matches(&[&s.id, &s.name]))
        .map(|s| json!({"asset": s.id, "name": s.name, "base": s.base}))
        .collect();
    Ok(json!({"species": species, "custom": custom}))
}

/// `date` and `season` of a light query.
#[allow(clippy::type_complexity)]
fn light_date(
    id: &str,
    params: &Value,
) -> Result<(Option<(u32, u32)>, Option<visioncraft_core::Season>), String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Date {
        month: u32,
        day: u32,
    }
    let date = take::<Date>(id, params, "date")?.map(|d| (d.month, d.day));
    let season = match take::<String>(id, params, "season")? {
        Some(s) => Some(visioncraft_core::Season::parse(&s).ok_or_else(|| {
            format!("{id}: `season` must be spring, summer, autumn or winter, not `{s}`")
        })?),
        None => None,
    };
    Ok((date, season))
}

/// `planting.add_border` on a host: `style: auto` picks the style from the sun the outline gets,
/// and an explicit style that clearly mismatches the measured light gets a `light_fit` warning.
fn add_border_host(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    use visioncraft_core::sun_hours::{FULL_SUN_HOURS, PART_SHADE_HOURS};
    let id = "planting.add_border";
    let style = params.get("style").and_then(Value::as_str).unwrap_or("");
    let auto = style == "auto";
    let wants_light = auto
        || (visioncraft_core::border::STYLES.contains(&style)
            && params.get("species").is_none_or(Value::is_null));
    let mut chosen = None;
    let mut measured = None;
    if wants_light {
        let (session, state) = host.parts();
        let outline = match (
            params.get("outline"),
            params.get("area").and_then(Value::as_str),
        ) {
            (Some(o), None) => serde_json::from_value::<Vec<[f32; 2]>>(o.clone()).ok(),
            (None, Some(a)) => match session.project().shapes.iter().find(|s| s.id() == a) {
                Some(visioncraft_core::Shape::Area(x)) => Some(x.outline().into_owned()),
                _ => None,
            },
            _ => None,
        }
        .filter(|o| o.len() >= 3 && o.iter().flatten().all(|c| c.is_finite()));
        match outline {
            Some(o) => match inspect::border_light(session, state, &o) {
                Ok(m) => measured = Some(m),
                Err(e) if auto => return Err(e.replace("query.plants_for_light", id)),
                Err(_) => {}
            },
            None if auto => {
                return Err(format!(
                    "{id}: style `auto` needs an `outline` (at least 3 points) or the `area` of an area shape, to measure its sun"
                ));
            }
            None => {}
        }
    }
    let mut params = std::borrow::Cow::Borrowed(params);
    if auto {
        let (mean, _) = measured.as_ref().expect("auto measured or returned");
        let dry = params.get("dry").and_then(Value::as_bool).unwrap_or(false);
        let picked = if f64::from(*mean) < PART_SHADE_HOURS {
            "shade"
        } else if f64::from(*mean) <= FULL_SUN_HOURS {
            "cottage"
        } else if dry {
            "dry"
        } else {
            "prairie"
        };
        params.to_mut()["style"] = json!(picked);
        chosen = Some(picked);
    }
    let mut result = run_core(host, id, &params)?;
    if let Some((mean, spot)) = measured {
        let style = chosen.unwrap_or(style);
        result["light"] = spot;
        if let Some(c) = chosen {
            result["style"] = json!(c);
            result["style_reason"] = json!(format!(
                "`auto`: the outline gets {mean:.1} h of direct sun on the summer solstice (shade under {PART_SHADE_HOURS} h, cottage up to {FULL_SUN_HOURS} h, prairie or dry above)"
            ));
        }
        let mismatch = match style {
            "shade" => f64::from(mean) >= FULL_SUN_HOURS,
            "cottage" | "prairie" | "dry" => f64::from(mean) < PART_SHADE_HOURS,
            _ => false,
        };
        let mut warnings = Vec::new();
        if mismatch {
            warnings.push(format!(
                "light_fit: the `{style}` style suits {} but the outline gets {mean:.1} h of direct sun on the summer solstice; try `style: auto`, or `query.plants_for_light` with this outline for species that fit",
                if style == "shade" { "under 6 h of sun" } else { "3 h of sun and more" }
            ));
        }
        result["warnings"] = json!(warnings);
    }
    Ok(result)
}

/// A copy of `project` with the fields of `time` changed, checked as `time.set` checks them.
fn with_time(project: &Project, time: &Value) -> Result<Project, String> {
    let mut scratch = Session::new(project.clone());
    scratch
        .run("time.set", time.clone())
        .map_err(|e| format!("`time`: {e}"))?;
    Ok(scratch.project().clone())
}

/// `view.render`'s photo: averaged frames, a lens or a shutter. Quality `photo` takes what it
/// is not given from the project's photo settings (`view.set_photo`); another quality averages
/// only when asked to (`samples` over 1, an aperture or a shutter time), else it is the still.
fn photo_request(
    id: &str,
    a: &Fields,
    quality: &str,
    settings: &visioncraft_core::PhotoSettings,
) -> Result<Option<PhotoRequest>, String> {
    use visioncraft_core::photo::{F_NUMBERS, FOCUS_M, Focus, SHUTTER_MAX_S};
    let photo = quality == "photo";
    let samples = a.integer("samples", 1, 1024)?.map(|n| n as u32);
    let aperture = match a.get("aperture") {
        None => photo.then_some(settings.aperture).flatten(),
        Some(Value::Null) => None,
        Some(v) => Some(
            v.as_f64()
                .map(|n| n as f32)
                .filter(|n| F_NUMBERS.contains(n))
                .ok_or_else(|| {
                    format!("{id}: `aperture` must be an f-number from 1 to 32, or null")
                })?,
        ),
    };
    let aperture_m = a.number("aperture_m")?;
    if aperture_m.is_some_and(|x| !(0.0..=0.5).contains(&x)) {
        return Err(format!("{id}: `aperture_m` must be from 0 to 0.5"));
    }
    if aperture_m.is_some() && a.get("aperture").is_some() {
        return Err(format!(
            "{id}: give `aperture` (an f-number) or `aperture_m`, not both"
        ));
    }
    let focus = match a.get("focus_m") {
        None if photo => settings.focus,
        None => Focus::Target,
        Some(v) => serde_json::from_value::<Focus>(v.clone())
            .ok()
            .filter(|f| match f {
                Focus::Metres(m) => FOCUS_M.contains(m),
                _ => true,
            })
            .ok_or_else(|| {
                format!("{id}: `focus_m` must be from 0.1 to 1000, \"auto\" or \"target\"")
            })?,
    };
    let focus_point = match a.get("focus_point") {
        None if photo => settings.focus_point,
        None => [0.5, 0.5],
        Some(v) => serde_json::from_value::<[f32; 2]>(v.clone())
            .ok()
            .filter(|p| p.iter().all(|c| (0.0..=1.0).contains(c)))
            .ok_or_else(|| format!("{id}: `focus_point` must be [x, y] from 0 to 1"))?,
    };
    let shutter_s = match a.number("shutter_s")? {
        Some(s) if !(0.0..=f64::from(SHUTTER_MAX_S)).contains(&s) => {
            return Err(format!("{id}: `shutter_s` must be from 0 to 1"));
        }
        Some(s) => s as f32,
        None if photo => settings.shutter_s,
        None => 0.0,
    };
    let aperture_m = aperture_m.unwrap_or(0.0) as f32;
    let samples = samples.unwrap_or(if photo { settings.samples } else { 1 });
    let lens = aperture.is_some() || aperture_m > 0.0;
    Ok(
        (photo || samples > 1 || lens || shutter_s > 0.0).then_some(PhotoRequest {
            samples,
            f_number: aperture,
            aperture_m,
            focus,
            focus_point,
            shutter_s,
        }),
    )
}

fn view_render(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let id = "view.render";
    let a = Fields::new(
        id,
        "parameter",
        Some(params),
        &[
            "path",
            "width",
            "height",
            "viewpoint",
            "camera",
            "time",
            "season",
            "snow",
            "wetness",
            "rain",
            "haze",
            "mist",
            "gustiness",
            "shafts",
            "weather",
            "weather_to",
            "weather_mix",
            "clouds",
            "lightning",
            "sky_occlusion",
            "white_balance",
            "grade",
            "grade_to",
            "grade_mix",
            "lights",
            "exposure_offset_ev",
            "quality",
            "shadow_filter",
            "shadow_radius_m",
            "years",
            "samples",
            "aperture",
            "aperture_m",
            "focus_m",
            "focus_point",
            "shutter_s",
            "vignette",
            "grain",
            "animation_s",
        ],
    )?;
    let animation_s = a.number("animation_s")?.unwrap_or(0.0);
    if !(0.0..=86_400.0).contains(&animation_s) {
        return Err(format!("{id}: `animation_s` must be from 0 to 86400"));
    }
    let width = a.integer("width", 16, 4096)?.unwrap_or(800) as u32;
    let height = a.integer("height", 16, 4096)?.unwrap_or(600) as u32;
    let quality = a.str("quality")?.unwrap_or("high");
    if !["low", "medium", "high", "ultra", "photo"].contains(&quality) {
        return Err(format!(
            "{id}: `quality` must be low, medium, high, ultra or photo, got `{}`",
            shorten(quality)
        ));
    }
    // The sun's shadow filter over the preset's, for this picture only.
    let shadow_taps = match a.str("shadow_filter")? {
        None => None,
        Some(name) => Some(match name {
            "hard" => 1,
            "pcf4" => 4,
            "pcf9" => 9,
            "pcf16" => 16,
            _ => {
                return Err(format!(
                    "{id}: `shadow_filter` must be hard, pcf4, pcf9 or pcf16, got `{}`",
                    shorten(name)
                ));
            }
        }),
    };
    let shadow_radius_m = a.number("shadow_radius_m")?;
    if shadow_radius_m.is_some_and(|r| !(0.0..=1.0).contains(&r)) {
        return Err(format!("{id}: `shadow_radius_m` must be from 0 to 1"));
    }
    // Only the 16-tap disk spreads over a radius: hard and the 4- and 9-tap grids have no use
    // for one.
    if shadow_radius_m.is_some() && !matches!(shadow_taps, None | Some(16)) {
        return Err(format!(
            "{id}: `shadow_radius_m` needs shadow_filter pcf16 (or none)"
        ));
    }
    let mut project = host.session().project().clone();
    let photo = photo_request(id, &a, quality, &project.view.photo)?;
    let mut camera = None;
    if let Some(v) = a.str("viewpoint")? {
        let vp = project
            .viewpoints
            .iter()
            .find(|x| x.id == v || x.name == v)
            .ok_or_else(|| {
                let names: Vec<String> = project
                    .viewpoints
                    .iter()
                    .map(|x| format!("{} ({})", x.id, x.name))
                    .collect();
                format!(
                    "{id}: no viewpoint `{}`; the viewpoints are: {}",
                    shorten(v),
                    if names.is_empty() {
                        "none".into()
                    } else {
                        names.join(", ")
                    }
                )
            })?
            .clone();
        camera = Some((vp.position, vp.target, vp.fov_y_deg));
        if let Some(t) = vp.time {
            project.time = t;
        }
        if vp.years.is_some() {
            project.view.years = vp.years;
        }
    }
    if let Some(y) = a.get("years") {
        let y = y
            .as_f64()
            .filter(|y| (0.0..=f64::from(visioncraft_core::YEARS_MAX)).contains(y))
            .ok_or_else(|| format!("{id}: `years` must be a number from 0 to 60"))?;
        project.view.years = Some(y as f32);
    }
    if let Some(c) = a.get("camera") {
        let pos = need::<[f32; 3]>(id, c, "position")?;
        let target = need::<[f32; 3]>(id, c, "target")?;
        let fov = take::<f32>(id, c, "fov_y_deg")?.unwrap_or(50.0);
        if pos == target || !(1.0..=179.0).contains(&fov) {
            return Err(format!(
                "{id}: `camera` needs position and target apart and fov_y_deg from 1 to 179"
            ));
        }
        camera = Some((pos, target, fov));
    }
    if let Some(t) = a.get("time") {
        project = with_time(&project, t).map_err(|e| format!("{id}: {e}"))?;
    }
    let season =
        match a.str("season")? {
            Some(s) => Some(visioncraft_core::Season::parse(s).ok_or_else(|| {
                format!("{id}: `season` must be spring, summer, autumn or winter")
            })?),
            None => None,
        };
    if let Some(s) = season {
        project.time = s.time(project.time, project.site.latitude);
    }
    if let Some(snow) = a.number("snow")? {
        if !(0.0..=1.0).contains(&snow) {
            return Err(format!("{id}: `snow` must be from 0 to 1"));
        }
        project.view.snow = snow as f32;
        // Snow asked for on its own brings winter; with a season or date of another time of
        // year it is refused rather than drawn under summer trees.
        if snow > 0.0 && season.is_none() && a.get("time").is_none() {
            project.time =
                visioncraft_core::Season::Winter.time(project.time, project.site.latitude);
        }
    }
    // A weather first, so the fields given with it win.
    let weather = |key: &str| -> Result<Option<visioncraft_core::Weather>, String> {
        a.str(key)?
            .map(|w| {
                visioncraft_core::Weather::parse(w).ok_or_else(|| {
                    format!(
                        "{id}: `{key}` must be one of {}",
                        visioncraft_core::WEATHERS.join(", ")
                    )
                })
            })
            .transpose()
    };
    let (from, to) = (weather("weather")?, weather("weather_to")?);
    let mix = a.number("weather_mix")?;
    if mix.is_some_and(|m| !(0.0..=1.0).contains(&m)) {
        return Err(format!("{id}: `weather_mix` must be from 0 to 1"));
    }
    if from.is_some() || to.is_some() {
        if to.is_none() && mix.is_some() {
            return Err(format!("{id}: `weather_mix` needs `weather_to`"));
        }
        let from = from.unwrap_or(visioncraft_core::Weather::Fair);
        let to = to.map(|w| (w, mix.unwrap_or(0.5) as f32));
        visioncraft_core::weather::apply(&mut project.view, from, to);
    }
    match a.get("clouds") {
        None | Some(Value::Null) => {}
        Some(Value::Number(n)) => {
            let cover = n
                .as_f64()
                .filter(|c| (0.0..=1.0).contains(c))
                .ok_or_else(|| format!("{id}: `clouds` as a cover must be from 0 to 1"))?;
            let clouds = &mut project.view.clouds;
            clouds.cover = cover as f32;
            if cover > 0.0 && clouds.kind == visioncraft_core::CloudKind::Clear {
                clouds.kind = visioncraft_core::CloudKind::Cumulus;
            }
        }
        Some(Value::Object(given)) => {
            let mut clouds = serde_json::to_value(project.view.clouds).unwrap_or_default();
            if let Some(c) = clouds.as_object_mut() {
                c.remove("blend");
                c.extend(given.clone());
            }
            let clouds: visioncraft_core::Clouds =
                serde_json::from_value(clouds).map_err(|e| format!("{id}: `clouds`: {e}"))?;
            clouds.validate().map_err(|e| format!("{id}: {e}"))?;
            project.view.clouds = clouds;
        }
        Some(_) => {
            return Err(format!(
                "{id}: `clouds` must be a cover 0..1 or an object as view.set takes it"
            ));
        }
    }
    for (key, field) in [
        ("wetness", &mut project.view.wetness),
        ("rain", &mut project.view.rain),
        ("mist", &mut project.view.mist),
        ("gustiness", &mut project.view.gustiness),
        ("shafts", &mut project.view.shafts),
        ("lightning", &mut project.view.clouds.lightning),
        ("vignette", &mut project.view.vignette),
        ("grain", &mut project.view.grain),
    ] {
        if let Some(v) = a.number(key)? {
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("{id}: `{key}` must be from 0 to 1"));
            }
            *field = v as f32;
        }
    }
    if let Some(v) = a.number("haze")? {
        let max = f64::from(visioncraft_core::film::HAZE_MAX);
        if !(0.0..=max).contains(&v) {
            return Err(format!("{id}: `haze` must be from 0 to {max}"));
        }
        project.view.haze = v as f32;
    }
    match a.get("sky_occlusion") {
        None | Some(Value::Null) => {}
        Some(Value::Bool(on)) => project.view.sky_occlusion = Some(*on),
        Some(_) => return Err(format!("{id}: `sky_occlusion` must be true or false")),
    }
    if let Some(v) = a.get("white_balance") {
        let wb: visioncraft_core::WhiteBalance = serde_json::from_value(v.clone())
            .map_err(|_| {
                format!("{id}: `white_balance` must be \"daylight\", \"auto\" or a temperature in kelvin")
            })?;
        if let visioncraft_core::WhiteBalance::Kelvin(k) = wb
            && !visioncraft_core::WHITE_BALANCE_KELVIN.contains(&k)
        {
            return Err(format!(
                "{id}: `white_balance` in kelvin must be from 2000 to 12000"
            ));
        }
        project.view.white_balance = wb;
    }
    if let Some(v) = a.str("grade")? {
        project.view.grade = serde_json::from_value(serde_json::json!(v)).map_err(|_| {
            format!(
                "{id}: `grade` must be neutral, warm, cool or soft, got `{}`",
                shorten(v)
            )
        })?;
    }
    // Lamps: a level 0..1 (a film's `lights` span fades them), true or false; any value forces
    // them on or off, else they follow the sun.
    let lamp_level = match a.get("lights") {
        None => 1.0,
        Some(Value::Bool(on)) => f32::from(u8::from(*on)),
        Some(v) => v
            .as_f64()
            .filter(|l| (0.0..=1.0).contains(l))
            .ok_or_else(|| format!("{id}: `lights` must be true, false or a number from 0 to 1"))?
            as f32,
    };
    if a.get("lights").is_some() {
        project.view.lights = Some(lamp_level > 0.0);
    }
    let exposure_offset_ev = match a.number("exposure_offset_ev")? {
        Some(e) if !(-8.0..=8.0).contains(&e) => {
            return Err(format!(
                "{id}: `exposure_offset_ev` must be from -8 to 8 stops"
            ));
        }
        Some(e) => e as f32,
        None => 0.0,
    };
    let grade_mix = match (a.str("grade_to")?, a.number("grade_mix")?) {
        (Some(to), mix) => {
            let grade: visioncraft_core::Grade =
                serde_json::from_value(json!(to)).map_err(|_| {
                    format!(
                        "{id}: `grade_to` must be neutral, warm, cool or soft, got `{}`",
                        shorten(to)
                    )
                })?;
            let mix = mix.unwrap_or(0.5);
            if !(0.0..=1.0).contains(&mix) {
                return Err(format!("{id}: `grade_mix` must be from 0 to 1"));
            }
            Some((grade, mix as f32))
        }
        (None, Some(_)) => return Err(format!("{id}: `grade_mix` goes with `grade_to`")),
        (None, None) => None,
    };
    if project.view.snow > 0.0 && project.season() != visioncraft_core::Season::Winter {
        return Err(format!(
            "{id}: snow lies only in winter, and this picture is in {}; give `season: \"winter\"` or `snow: 0`",
            project.season().name()
        ));
    }
    let path = match a.str("path")? {
        Some(p) => Some(
            host.state()
                .access
                .resolve(p, true, None)
                .map_err(|e| format!("{id}: {e}"))?,
        ),
        None => None,
    };
    let request = RenderRequest {
        width,
        height,
        camera,
        quality: quality.to_string(),
        photo,
        animation_s,
        lamp_level,
        exposure_offset_ev,
        grade_mix,
        shadow_taps,
        shadow_radius_m: shadow_radius_m.map(|r| r as f32),
        images: {
            let state = host.state();
            project
                .image_files()
                .into_iter()
                .filter_map(|f| Some((f.to_string(), state.images.get(f)?.clone())))
                .collect()
        },
        assets: host.state().catalog.assets.clone(),
    };
    let models = host.state().models.clone();
    let rendered = host
        .render(&project, &models, &request)
        .map_err(|e| format!("{id}: {e}"))?;
    let mut out = match rendered.info {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    out.insert("width".into(), width.into());
    out.insert("height".into(), height.into());
    out.insert("time".into(), json!(project.time));
    out.insert("years".into(), json!(project.view.years));
    out.insert("sun".into(), inspect::sun_json(&project));
    match path {
        Some(path) => {
            std::fs::write(&path, &rendered.png)
                .map_err(|e| format!("{id}: cannot write `{}`: {e}", path.display()))?;
            out.insert("path".into(), path.display().to_string().into());
            out.insert("bytes".into(), rendered.png.len().into());
        }
        None => {
            out.insert("mime".into(), "image/png".into());
            out.insert("base64".into(), crate::base64(&rendered.png).into());
        }
    }
    Ok(Value::Object(out))
}

// ------------------------------------------------------------------------- agent log

/// Most entries the agent log keeps; the oldest go first (3dcraft's `MAX_ENTRIES`).
pub const MAX_LOG_ENTRIES: usize = 500;
/// Most characters of an intent; a longer one is cut with `…`.
pub const MAX_INTENT_CHARS: usize = 300;

/// An intent as the log keeps it: control characters and runs of white space become one space,
/// direction marks and invisible characters go, at most 300 characters. `None` when empty.
/// Ported from 3dcraft `crates/engine/src/agent_log.rs` (`clean_intent`, MIT).
pub fn clean_intent(text: &str) -> Option<String> {
    let invisible = |c: char| matches!(c, '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}');
    let mut out = String::new();
    for c in text.chars().filter(|c| !invisible(*c)) {
        if c.is_control() || c.is_whitespace() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    let out = out.trim_end().to_string();
    if out.is_empty() {
        return None;
    }
    if out.chars().count() > MAX_INTENT_CHARS {
        let cut: String = out.chars().take(MAX_INTENT_CHARS - 1).collect();
        return Some(format!("{}…", cut.trim_end()));
    }
    Some(out)
}

/// Runs a command for an agent and notes it in the agent log (`agent.log`), as 3dcraft's
/// window does: who (the MCP client's name), when, the command, how it went, what it made or
/// changed, and the intent. Reading commands (`agent.log`, `document.inspect`, `edit.state`) are not noted.
pub fn execute_logged(
    host: &mut dyn Host,
    id: &str,
    params: &Value,
    intent: Option<&str>,
) -> Result<Value, String> {
    let outcome = execute(host, id, params);
    if matches!(id, "agent.log" | "document.inspect" | "edit.state") {
        return outcome;
    }
    let label = docs(host.session())
        .into_iter()
        .find(|d| d.id == id)
        .map_or_else(|| id.to_string(), |d| d.label);
    let state = host.state();
    let (ok, message) = match &outcome {
        Ok(v) => (true, v.to_string()),
        Err(e) => (false, e.clone()),
    };
    let message: String = message.chars().take(160).collect();
    let created: Vec<&str> = outcome
        .as_ref()
        .ok()
        .and_then(|v| v["id"].as_str())
        .into_iter()
        .collect();
    let changed: Vec<&str> = params["id"].as_str().into_iter().collect();
    // The browser's std has no clock: `SystemTime::now` panics there, so it is not even called.
    #[cfg(not(target_arch = "wasm32"))]
    let time_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    #[cfg(target_arch = "wasm32")]
    let time_ms = 0u64;
    state.log.push_back(json!({
        "client": state.client.clone().unwrap_or_else(|| "agent".into()),
        "timeMs": time_ms,
        "command": id, "label": label, "ok": ok, "message": message,
        "created": created, "changed": changed, "intent": intent.and_then(clean_intent),
    }));
    while state.log.len() > MAX_LOG_ENTRIES {
        state.log.pop_front();
    }
    outcome
}

fn agent_log(host: &mut dyn Host, params: &Value) -> Result<Value, String> {
    let a = Fields::new("agent.log", "parameter", Some(params), &["limit"])?;
    let limit = a.integer("limit", 1, MAX_LOG_ENTRIES as u64)?.unwrap_or(50) as usize;
    let log = &host.state().log;
    let entries: Vec<Value> = log.iter().rev().take(limit).cloned().collect();
    Ok(json!({"total": log.len(), "entries": entries}))
}

// ---------------------------------------------------------------------------- batches

/// One step of a batch.
pub struct Step {
    pub command: String,
    pub params: Value,
    /// Why this step, when it differs from the batch's intent.
    pub intent: Option<String>,
}

/// Reads and checks every step before any runs. `id_key` is what the protocol calls the
/// command in a step (`command` in JSON lines, `id` in MCP).
pub(crate) fn parse_steps(
    owner: &str,
    noun: &str,
    fields: &Fields,
    id_key: &str,
) -> Result<Vec<Step>, String> {
    let items = fields
        .array("steps")?
        .ok_or_else(|| fields.missing("steps"))?;
    if items.len() > MAX_BATCH_STEPS {
        return Err(format!(
            "{owner}: `steps` has {} steps, and a batch takes at most {MAX_BATCH_STEPS}; split it into several batches",
            items.len()
        ));
    }
    let mut steps = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let step_owner = format!("{owner}: steps[{index}]");
        if !item.is_object() {
            return Err(format!(
                "{step_owner} must be an object {{\"{id_key}\": string, \"params\": object}}, got {}",
                describe(item)
            ));
        }
        let step = Fields::new(&step_owner, noun, Some(item), &[id_key, "params", "intent"])?;
        steps.push(Step {
            command: step.required_str(id_key)?.to_string(),
            params: step.object("params")?,
            intent: step.str("intent")?.map(str::to_string),
        });
    }
    Ok(steps)
}

/// How a batch runs (3dcraft's `BatchStops` and `terse`, plus `atomic`).
#[derive(Clone, Copy)]
pub struct BatchOptions<'a> {
    /// Stop at the first command that returns an error (default true).
    pub stop_on_error: bool,
    /// Also stop at the first step that makes something fail. Accepted for 3dcraft's shape:
    /// a VisionCraft project has no features that fail, so no step breaks anything.
    pub stop_on_break: bool,
    pub terse: bool,
    /// All or nothing (VisionCraft's extra): see [`run_batch`].
    pub atomic: bool,
    pub intent: Option<&'a str>,
}

impl Default for BatchOptions<'_> {
    fn default() -> Self {
        Self {
            stop_on_error: true,
            stop_on_break: false,
            terse: false,
            atomic: false,
            intent: None,
        }
    }
}

/// The outcome of a batch: the report both protocols return, and whether it failed.
pub struct BatchOutcome {
    pub report: Value,
    pub failed: bool,
}

/// Runs the steps in order, as 3dcraft does: not a transaction, each step its own undo step;
/// `failed` counts the commands that returned an error, and `stop_on_error` stops at the first.
/// Report: `{completed, failed, results: [{ok, result | error, ms}], ms}`, or with `terse`
/// `{completed, failed, errors: [{step, error, ms}], last: {step, result, ms}, ms}`.
///
/// With `atomic` the steps run on a scratch copy of the project and the first failure stops
/// the batch and takes back everything it did: the project, its history and the open file are
/// as before (`rolledBack: true`). An atomic batch that succeeds is one undo step. Files written
/// by a step (`file.save`, `view.render` with a path) stay written either way.
pub fn run_batch(host: &mut dyn Host, steps: &[Step], options: &BatchOptions) -> BatchOutcome {
    let whole = Stopwatch::start();
    let mut report = if options.atomic {
        atomic(host, steps, options)
    } else {
        run_steps(host, steps, options)
    };
    let failed = report["failed"].as_u64().unwrap_or(0) > 0;
    report["ms"] = whole.ms();
    BatchOutcome { report, failed }
}

/// Commands that replace the session or step through its history: they cannot be part of a
/// transaction.
const NOT_ATOMIC: [&str; 4] = ["file.new", "file.open", "edit.undo", "edit.redo"];

/// The steps inside one core transaction (`Session::transaction`): the first failure takes back
/// everything, success is one undo step, commands registered by extensions take part.
fn atomic(host: &mut dyn Host, steps: &[Step], options: &BatchOptions) -> Value {
    if let Some(step) = steps
        .iter()
        .find(|s| NOT_ATOMIC.contains(&s.command.as_str()))
    {
        let error = format!(
            "{}: cannot be part of an atomic batch; run it on its own",
            step.command
        );
        let mut report = json!({"completed": 0, "failed": 1, "rolledBack": true});
        if options.terse {
            report["errors"] = json!([{"step": 0, "error": error, "ms": Value::Null}]);
            report["last"] = Value::Null;
        } else {
            report["results"] = json!([{"ok": false, "error": error, "ms": Value::Null}]);
        }
        return report;
    }
    let saved = {
        let state = host.state();
        (
            state.file.clone(),
            state.models.clone(),
            state.missing.clone(),
            state.saved.clone(),
        )
    };
    // The session leaves the host for the transaction and comes back inside it, so the steps
    // run on the host as always.
    let mut session = std::mem::replace(host.session(), Session::new(Project::default()));
    let mut report = Value::Null;
    let _ = session.transaction(|inside| {
        std::mem::swap(host.session(), inside);
        report = run_steps(
            host,
            steps,
            &BatchOptions {
                stop_on_error: true,
                ..*options
            },
        );
        std::mem::swap(host.session(), inside);
        if report["failed"].as_u64().unwrap_or(0) > 0 {
            Err(visioncraft_core::CommandError::Failed("rolled back".into()))
        } else {
            Ok(())
        }
    });
    *host.session() = session;
    let rolled_back = report["failed"].as_u64().unwrap_or(0) > 0;
    if rolled_back {
        let state = host.state();
        (state.file, state.models, state.missing, state.saved) = saved;
        let project = host.session().project().clone();
        host.state().sync_models(&project);
    }
    report["rolledBack"] = rolled_back.into();
    report
}

fn run_steps(host: &mut dyn Host, steps: &[Step], options: &BatchOptions) -> Value {
    let (mut results, mut errors, mut last) = (Vec::new(), Vec::new(), Value::Null);
    let (mut completed, mut failed) = (0usize, 0usize);
    for (index, step) in steps.iter().enumerate() {
        let clock = Stopwatch::start();
        let intent = step.intent.as_deref().or(options.intent);
        // A panic must not leave the session out of the host (atomic batches).
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute_logged(host, &step.command, &step.params, intent)
        }))
        .unwrap_or_else(|payload| {
            Err(format!(
                "{}: internal error (a bug): {}",
                step.command,
                crate::rpc::panic_text(payload.as_ref())
            ))
        });
        let ms = clock.ms();
        match outcome {
            Ok(result) => {
                completed += 1;
                if options.terse {
                    last = json!({"step": index, "result": result, "ms": ms});
                } else {
                    results.push(json!({"ok": true, "result": result, "ms": ms}));
                }
            }
            Err(error) => {
                failed += 1;
                if options.terse {
                    errors.push(json!({"step": index, "error": error, "ms": ms}));
                } else {
                    results.push(json!({"ok": false, "error": error, "ms": ms}));
                }
                if options.stop_on_error {
                    break;
                }
            }
        }
    }
    let mut report = json!({"completed": completed, "failed": failed});
    if options.terse {
        report["errors"] = errors.into();
        report["last"] = last;
    } else {
        report["results"] = results.into();
    }
    report
}
