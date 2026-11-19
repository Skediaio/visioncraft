use super::{fail, next_id};
use crate::project::{Layer, Project};
use crate::session::{CommandError, Session, parse};
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "layer.add",
            "Add Layer",
            "Adds a visible layer that models, shapes, generators and lights can sit on. With `context` the layer holds surroundings (a neighbour's house as a closed extrusion, an imported model): they cast and receive shadow but are not part of the design, draw in one neutral massing look and stay out of `query.quantities`.",
            vec![
                param("name", T::String, "Display name.").required(),
                param("context", T::Boolean, "True for a surroundings layer.").default(json!(false)),
            ],
            "{id}",
            json!({"name": "Neighbours", "context": true}),
        ),
        layer_add,
    );
    s.register_with_spec(
        spec(
            "layer.rename",
            "Rename Layer",
            "Renames a layer.",
            vec![
                param("id", T::String, "Layer id.").required(),
                param("name", T::String, "New display name.").required(),
            ],
            "null",
            json!({"id": "shapes", "name": "Drawn shapes"}),
        ),
        layer_rename,
    );
    s.register_with_spec(
        spec(
            "layer.set_visible",
            "Show or Hide Layer",
            "Shows or hides everything on a layer.",
            vec![
                param("id", T::String, "Layer id.").required(),
                param("visible", T::Boolean, "True to show.").required(),
            ],
            "null",
            json!({"id": "planting", "visible": false}),
        ),
        layer_set_visible,
    );
    s.register_with_spec(
        spec(
            "layer.set_context",
            "Mark Layer as Surroundings",
            "Marks a layer as surroundings (true) or as part of the design (false). Everything on a context layer, such as a neighbour's house, still casts and receives shadow but draws in one neutral massing look whatever its materials, and `query.quantities` leaves it out. Plantings on it draw normally.",
            vec![
                param("id", T::String, "Layer id.").required(),
                param("context", T::Boolean, "True for surroundings.").required(),
            ],
            "null",
            json!({"id": "l1", "context": true}),
        ),
        layer_set_context,
    );
    s.register_with_spec(
        spec(
            "layer.remove",
            "Remove Layer",
            "Removes a layer; refused while any model, shape, generator or light sits on it.",
            vec![param("id", T::String, "Layer id.").required()],
            "null",
            json!({"id": "l1"}),
        ),
        layer_remove,
    );
    s.register_with_spec(
        spec(
            "layer.assign",
            "Move to Layer",
            "Moves a model, shape, generator, light or underlay to another layer.",
            vec![
                param(
                    "item",
                    T::String,
                    "Id of the model, shape, generator or light.",
                )
                .required(),
                param("layer", T::String, "Id of the layer to move it to.").required(),
            ],
            "null",
            json!({"item": "S1", "layer": "l1"}),
        ),
        layer_assign,
    );
    s.register_with_spec(
        spec(
            "style.set",
            "Set Render Style",
            "Switches the presentation mode that draws every surface white (clay).",
            vec![param("clay", T::Boolean, "True for clay mode.").required()],
            "null",
            json!({"clay": true}),
        ),
        style_set,
    );
    s.register_with_spec(
        spec(
            "view.set",
            "Set View",
            "Sets the season, the weather, snow, weathering, wetness, rain, mist, light shafts, exposure, white balance, grade, vignette, film grain, look, sky, clouds, years after planting, haze, wind and gusts, the ground plane, the surroundings, ground clutter, sky occlusion and the view distance stored with the project; only the fields given change.",
            vec![
                param("season", T::String, "spring, summer, autumn or winter: the plants as they are then, and the project's date set to the season's day at the site (20 April, 21 June, 20 October, 15 January in the north; six months on in the south), the hour kept. An exact date (time.set) maps back to its season."),
                param("weather", T::String, "A weather as one preset of the clouds, rain, wetness, mist, gusts and wind speed: clear (no clouds, still air), fair (cumulus over a third of the sky, a light breeze), cloudy (cumulus over most of it, a fresh breeze with gusts), overcast (a closed grey layer, soft shadows), rain (a low rain sky, steady rain, everything wet) or storm (towering storm clouds, a downpour, strong gusting wind). It writes those fields (`clouds` kind and cover, `rain`, `wetness`, `mist`, `gustiness`, `wind_speed_mps`); any of them given in the same call wins, and each can be changed on its own afterwards. The live view eases into it over `weather_transition_s`."),
                param("weather_transition_s", T::Number, "How long the live view takes to ease into a change of the clouds, rain, wetness, mist, gusts and wind, 0 to 600 s: a sky clears or closes in instead of switching. Pictures (`view.render`) always show the stored weather; films ease with their shots' `weather`. 0: at once.").unit("s").default(json!(0)),
                param("snow", T::Number, "Snow cover 0..1 on the ground, lawns, roofs, terraces, hedge tops, evergreen crowns, branches, wall tops and sills, not under roofs or dense evergreens; about 0.7 for a snowy winter. 0: none.").default(json!(0)),
                param("weathering", T::Number, "Weathering 0..1 of every standard material at once: dirt, moss, wear and rain streaks with the default mix (see material.update's `weathering`), added to each material's own. Keep it explicit: `years` does not drive it. 0: none.").default(json!(0)),
                param("wetness", T::Number, "Wet surfaces 0..1: porous materials (soil, wood, stone, concrete, brick, fabric) darken and turn glossy, leaves take a sheen, metal and glass barely change; above about 0.5 puddles form on flat, open, upward-facing paving, terraces, paths and soil (not on lawns, plants, steep surfaces or under roofs), growing with the value. Rain implies it: the wetness drawn is the larger of `wetness` and `rain`. 0: dry.").default(json!(0)),
                param("rain", T::Number, "Rain 0..1: falling streaks slanted by the wind, ripple rings on puddles and ponds, an overcast sky and a weaker sun, and everything wet as `wetness` of the same value. About 0.4 a drizzle, 0.8 a downpour. Rain also brings fog: the visibility falls to about 1.1 km in a drizzle and 400 m in a downpour (distant hedges and the horizon fade into grey, the garden 10-30 m off only softens). 0: none.").default(json!(0)),
                param("mist", T::Number, "Mist 0..1: a low layer of droplets over the ground, under the haze, thinning with a scale height of 25 m. The visibility falls from 1.3 km at 0.3 (a light morning haze) to 330 m at 0.6 (a misty morning) and 120 m at 1 (fog); the distance and the horizon fade into a white veil (grey under a rain sky), the near garden softens, the sun glows through it and is dimmed on the ground. Adds to the fog rain brings. 0: none.").default(json!(0)),
                param("shafts", T::Number, "Light shafts 0..1: the sun's light in the haze, mist and rain fog is shadowed by crowns, hedges and houses (a march through the four shadow cascades with the fog's own density and phase), so soft beams and darker lanes run through the air where the sun shines through gaps; strongest looking towards a low sun. 1 is the physical amount. They need a medium: mist 0.2-0.4, rain or thick haze; in clear air nothing shows (and nothing is computed). 0: none.").default(json!(0)),
                param("white_balance", T::Any, "The camera's white balance: \"daylight\" (fixed at 6500 K: a low sun reads orange, as on film), \"auto\" (from the sun's and the sky's light at the time shown, as a camera's automatic white balance: most of a warm or cool cast is taken out, some of the mood kept) or a colour temperature in kelvin, 2000 to 12000 (light of that temperature reads white: 3200 makes daylight blue, 9000 warms an overcast day).").default(json!("daylight")),
                param("grade", T::String, "A colour grade before the tone map: neutral (none), warm (a little warmer and richer), cool (a little cooler and calmer) or soft (lower contrast and colour, a matte print).").default(json!("neutral")),
                param("vignette", T::Number, "The lens' light fall-off towards the corners 0..1, before the tone map (highlights stay bright): 0.3 a light vignette (about half a stop off the corners), 1 two stops. In the live view and every picture. 0: none.").default(json!(0)),
                param("grain", T::Number, "Film grain 0..1 after the tone map: noise in the luminance only (no colour speckle), most in the mid-tones, none in black; 0.2 is a fine grain, 0.5 a fast film. In the live view and every picture. 0: none.").default(json!(0)),
                param("exposure_ev", T::Number, "A fixed exposure as an EV100 value, not a correction: lower is brighter (about 15 in full sun, 13 under a low sun), and 0 is a white frame in daylight; null for automatic exposure, the default. For stops relative to the automatic exposure (0 = no correction) use a film shot's `exposure_ev` or view.render's `exposure_offset_ev`."),
                param("sky", T::Object, "{type: \"physical\"} or {type: \"hdri\", id: string, rotation_deg: number, intensity: number >= 0}.")
                    .default(json!({"type": "physical"})),
                param("haze", T::Number, "Atmospheric haze as a turbidity: 1 pure air, 2.5 a clear day (about 30 km visibility), 6 hazy (9 km), 20 misty (2.5 km); 0 turns the haze off.").default(json!(2.5)),
                param("wind_speed_mps", T::Number, "Wind speed in m/s, 0 (perfectly still) to 60: plants lean with the drag (growing with the speed squared, saturating as crowns streamline), leaves flutter faster as it rises.").unit("m/s").default(json!(2.0)),
                param("wind_beaufort", T::Any, "A Beaufort step as a preset for `wind_speed_mps` (v = 0.836 B^1.5 m/s): 0-10 or its name. 0 calm (smoke rises), 1 light air, 2 light breeze (leaves rustle), 3 gentle breeze (leaves and twigs in constant motion), 4 moderate breeze (small branches move), 5 fresh breeze (small trees begin to sway), 6 strong breeze (large branches move), 7 near gale (whole trees move), 8 gale (twigs break off), 9 strong gale, 10 storm. Not together with `wind_speed_mps`."),
                param("wind_gustiness", T::Number, "How much the wind's strength swells and fades in a slow wave travelling downwind, 0 (steady) to 1.").default(json!(0.5)),
                param("gustiness", T::Number, "Gusts 0..1: patches of stronger wind (tens of metres across) travelling downwind at the wind speed. They roll over lawns as waves of bending grass whose tips turn paler, sway trunks, branches and leaves of one crown after another, and swing the rain's slant; at 1 a gust's core bends plants about 2.5 times as far as the mean wind, a lull a little over half as far. Scales with `wind_speed_mps`. 0: none (only the swell of `wind_gustiness`).").default(json!(0)),
                param("wind_strength", T::Number, "Deprecated: the relative wind of older projects (1 is 6.5 m/s); use `wind_speed_mps`."),
                param("wind_direction_deg", T::Number, "Direction the wind blows towards, true bearing clockwise from north seen from above (drawing -Z unless site.set north_deg turns it).").unit("deg").default(json!(0)),
                param("ground", T::Object, "Ground plane {height: m, color: [r, g, b] linear}; null for none.")
                    .default(json!({"height": 0, "color": [0.065, 0.12, 0.035]})),
                param("clouds", T::Object, "The clouds over the analytic sky, merged into the current ones (give only what changes): {kind: \"clear\" | \"cumulus\" (fair-weather clouds) | \"overcast\" (a closed grey layer: soft shadows, little sun) | \"storm\" (towering dark cumulonimbus, the least sun), cover: 0..1, altitude_m: the base, 300 to 5000 (1500; a closed or stormy sky lowers it), speed_mps: their drift downwind, 0 to 60 (null: faster than the wind at the ground), volumetric: true (volumetric clouds, lit by the sun and the sky, whose shadows drift over the ground; MEDIUM quality and up) or false (the flat layer), sun_gap: true (a gap in broken clouds in front of the sun over the site, so pictures stay sunny) or false (their shadows drift over the design too), lightning: how often lightning flashes in a storm sky, 0 (never) to 1 (every few seconds; 0.5), lighting the clouds from inside and the garden for a moment, now and then with a bolt far off, the same flashes at the same animation time}.").default(json!({"kind": "cumulus", "cover": 0.35})),
                param("lights", T::Boolean, "Lamps and lit rooms on (true) or off (false); null: on while the sun is below the horizon at the time shown."),
                param("sky_occlusion", T::Boolean, "Sky occlusion: the sky's light on every surface in proportion to the sky it sees, baked over the design (buildings, shapes, models, terrain, and crowns by their species' leaf density): the ground under trees and the corners by walls and the house go gently darker, the inside of a backlit crown keeps the sky that comes through its leaves, and wet paving and windows under a tree stop mirroring the open sky. Rebaked when the design, the years or the leaves change. null: the host decides (on in the window and its pictures)."),
                param("clutter", T::Boolean, "Ground clutter: fallen leaves under deciduous trees (most in October and November), stones and twigs on soil and gravel, grass hanging over the lawn's edge. Off at LOW quality whatever this says.").default(json!(true)),
                param("years", T::Number, "Years after planting to show the plants at (0..60): each grows from its planting age along its species' curve; null shows everything full grown.").unit("years"),
                param("look", T::String, "neutral, natural or vivid: the picture's exposure, contrast and colour on top of the tone map.").default(json!("natural")),
                param("surroundings", T::String, "The land round the design on the endless ground, beyond its plot: fields (farmland with hedgerows, woods and distant hills), woodland, dunes, or none (a plain plane).").default(json!("fields")),
                param("view_distance", T::Number, "How far plants, shadows and a clear view reach, as a multiple of the default; 0.25 to 4.")
                    .unit("x")
                    .default(json!(1.0)),
            ],
            "null",
            json!({"haze": 3.5, "wind_speed_mps": 3.3, "ground": null, "view_distance": 2.0}),
        ),
        view_set,
    );
    s.register_with_spec(
        spec(
            "view.set_photo",
            "Set Photo Settings",
            "Sets how a photo is taken, stored with the project: the frames averaged, the lens' aperture and focus, and the shutter time; only the fields given change. The app's photo export takes them, and so does `view.render` at quality `photo` for every field it is not given. Each frame is moved by a fraction of a pixel (smooth edges, no stair steps or shimmering foliage), the sun's light within its disc (soft shadows) and the noise of occlusion, light shafts and leaf edges by the frame, and their mean goes through bloom and the tone map once.",
            vec![
                param("samples", T::Integer, "Frames averaged, 1 to 1024: 16 smooth edges, 64 a clean photo, 256 for a wide aperture or long shutter.").default(json!(64)),
                param("aperture", T::Number, "The lens' aperture as an f-number, 1 to 32 (the focal length is the one a 36 mm full-frame sensor needs for the camera's field of view: 50 degrees is a 39 mm lens at 3:2): f/2.8 blurs the background behind a subject a few metres off, f/8 keeps a garden sharp from 2 m to the horizon. null: a pinhole, everything sharp."),
                param("focus", T::Any, "Where the lens is focused: \"target\" (the camera's target), \"auto\" (what the picture shows at `focus_point`, as a camera's autofocus) or a distance along the view in metres, 0.1 to 1000.").default(json!("target")),
                param("focus_point", T::Any, "[x, y], 0..1 from the picture's top left: where the autofocus looks.").default(json!([0.5, 0.5])),
                param("shutter_s", T::Number, "Shutter time 0 to 1: plants in the wind, water and falling rain blur with their motion over it (1/60 s = 0.0167 a hand-held shot). 0: a frozen instant.").unit("s").default(json!(0)),
            ],
            "null",
            json!({"samples": 64, "aperture": 2.8, "focus": "auto", "focus_point": [0.5, 0.6]}),
        ),
        view_set_photo,
    );
    s.register_with_spec(
        spec(
            "view.set_sun_map",
            "Show Sun Hours on the Ground",
            "Draws the hours of direct sun a day gives over an area on the ground, as a colour map (deep violet: no sun, through blue, teal and green to amber at 8 hours or more) lit like no surface: flat and see-through, a few centimetres over the area, in the app and in `view.render` pictures. The sun is cast as `query.sun_hours` does (models, shapes, terrain and plant crowns, `years` after planting). Give the area as `shape` (an area's id: the map follows it) or `outline`; when a map is already on, only the fields given change (a new target replaces the old, `date` and `season` replace each other, null clears `date`, `season` or `years`). Cells under another area drawn over the target (the pond or a path on the lawn: areas later in the shape list that overlap it) are left clear and out of the shares and stats; with an `outline` only water areas are left out. On terrain the map is a flat sheet at the area's height. Undo takes it away again.",
            vec![
                param("shape", T::String, "An area shape's id (S1...); the map covers its outline."),
                param("outline", T::Outline, "Or a polygon [[x, z], ...] in metres, for example the box round the plot. Not together with `shape`."),
                param("date", T::Object, "{month, day}: the day studied. Not together with `season`. Neither: the project's date."),
                param("season", T::String, "spring, summer, autumn or winter: studied on its solstice or equinox (21 March, 21 June, 21 September, 21 December in the north). Not together with `date`."),
                param("years", T::Number, "Years after planting the crowns have grown, 0..60; null follows the view's years (else 0).").unit("years"),
                param("cell_m", T::Number, "Cell size, 0.1 to 10; at most 10,000 cells inside the area are sampled.").unit("m").default(json!(0.5)),
                param("opacity", T::Number, "0 (invisible) to 1 (opaque).").default(json!(0.6)),
            ],
            "null",
            json!({"shape": "S1", "season": "winter", "years": 10}),
        ),
        view_set_sun_map,
    );
    s.register_with_spec(
        spec(
            "view.clear_sun_map",
            "Hide Sun Hours",
            "Takes the sun-hours map off the ground.",
            vec![],
            "null",
            json!({}),
        ),
        view_clear_sun_map,
    );
}

fn layer<'a>(project: &'a mut Project, id: &str) -> Result<&'a mut Layer, CommandError> {
    project
        .layers
        .iter_mut()
        .find(|l| l.id == id)
        .ok_or_else(|| fail(format!("no layer `{id}`")))
}

fn layer_add(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        name: String,
        #[serde(default)]
        context: bool,
    }
    let p: P = parse(params)?;
    let id = next_id("l", |id| project.layers.iter().any(|l| l.id == id));
    project.layers.push(Layer {
        id: id.clone(),
        name: p.name,
        visible: true,
        context: p.context,
    });
    Ok(json!({ "id": id }))
}

fn layer_rename(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        name: String,
    }
    let p: P = parse(params)?;
    layer(project, &p.id)?.name = p.name;
    Ok(Value::Null)
}

fn layer_set_visible(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        visible: bool,
    }
    let p: P = parse(params)?;
    layer(project, &p.id)?.visible = p.visible;
    Ok(Value::Null)
}

fn layer_set_context(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        context: bool,
    }
    let p: P = parse(params)?;
    layer(project, &p.id)?.context = p.context;
    Ok(Value::Null)
}

fn layer_remove(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
    }
    let p: P = parse(params)?;
    layer(project, &p.id)?;
    if project.layer_in_use(&p.id) {
        return Err(fail(format!("layer `{}` still has items on it", p.id)));
    }
    project.layers.retain(|l| l.id != p.id);
    Ok(Value::Null)
}

fn layer_assign(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        item: String,
        layer: String,
    }
    let p: P = parse(params)?;
    super::check_layer(project, &p.layer)?;
    *project
        .layer_of_mut(&p.item)
        .ok_or_else(|| fail(format!("no model, shape, generator or light `{}`", p.item)))? =
        p.layer;
    Ok(Value::Null)
}

fn style_set(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        clay: bool,
    }
    project.style.clay = parse::<P>(params)?.clay;
    Ok(Value::Null)
}

fn view_set(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    // The season is the date's, not a view field: it moves the project's date.
    let mut params = params.clone();
    let season = match params.as_object_mut().and_then(|o| o.remove("season")) {
        None => None,
        Some(v) => Some(v.as_str().and_then(crate::Season::parse).ok_or_else(|| {
            crate::session::bad("`season` must be spring, summer, autumn or winter")
        })?),
    };
    // Presets and the deprecated relative strength become a speed.
    let mut speed = None;
    if let Some(o) = params.as_object_mut() {
        let beaufort = o.remove("wind_beaufort");
        let relative = o.remove("wind_strength");
        if beaufort.is_some() && (o.contains_key("wind_speed_mps") || relative.is_some()) {
            return Err(crate::session::bad(
                "`wind_beaufort` sets the speed: not together with `wind_speed_mps`",
            ));
        }
        if let Some(b) = beaufort {
            let force = crate::wind::parse_beaufort(&b).ok_or_else(|| {
                crate::session::bad("`wind_beaufort` must be 0 to 10 or a step's name (\"gale\")")
            })?;
            speed = Some(crate::wind::beaufort_mps(force));
        }
        if let Some(r) = relative {
            let r = r.as_f64().filter(|r| *r >= 0.0).ok_or_else(|| {
                crate::session::bad("`wind_strength` must be a number, 0 or more")
            })?;
            speed = Some((r as f32 * crate::wind::RELATIVE_MPS).min(crate::wind::MAX_SPEED_MPS));
        }
    }
    if let (Some(v), Some(o)) = (speed, params.as_object_mut()) {
        o.insert("wind_speed_mps".into(), json!(v));
    }
    // A weather writes its fields first; the fields given with it win.
    let mut base = project.view.clone();
    if let Some(w) = params.as_object_mut().and_then(|o| o.remove("weather")) {
        let weather = w.as_str().and_then(crate::Weather::parse).ok_or_else(|| {
            crate::session::bad(&format!(
                "`weather` must be one of {}",
                crate::WEATHERS.join(", ")
            ))
        })?;
        crate::weather::apply(&mut base, weather, None);
    }
    // The clouds merge into the current ones; setting them directly ends a weather's mix.
    if let Some(Value::Object(given)) = params.get("clouds").cloned() {
        let mut clouds = serde_json::to_value(base.clouds).unwrap_or_default();
        if let Some(c) = clouds.as_object_mut() {
            c.remove("blend");
            c.extend(given);
        }
        if let Some(o) = params.as_object_mut() {
            o.insert("clouds".into(), clouds);
        }
    }
    // `view_distance` is not written while it is 1, `look` while natural, `clouds` while default.
    let view = super::merge_update(
        &base,
        &params,
        &[
            "view_distance",
            "look",
            "clouds",
            "weather_transition_s",
            "years",
            "lights",
            "clutter",
            "sky_occlusion",
            "snow",
            "weathering",
            "wetness",
            "rain",
            "mist",
            "gustiness",
            "shafts",
            "white_balance",
            "grade",
            "vignette",
            "grain",
            "photo",
            "surroundings",
        ],
    )?;
    view.validate().map_err(|e| crate::session::bad(&e))?;
    let time = season.map_or(project.time, |s| {
        s.time(project.time, project.site.latitude)
    });
    if view.snow > 0.0
        && crate::Season::of(time.month, project.site.latitude) != crate::Season::Winter
    {
        return Err(crate::session::bad(
            "snow lies only in winter: give `season: \"winter\"` with it, or `snow: 0` first",
        ));
    }
    project.view = view;
    project.time = time;
    Ok(Value::Null)
}

fn view_set_photo(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let photo: crate::photo::PhotoSettings =
        super::merge_update(&project.view.photo, params, &["aperture"])?;
    photo.validate().map_err(|e| crate::session::bad(&e))?;
    project.view.photo = photo;
    Ok(Value::Null)
}

fn view_set_sun_map(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    let Some(given) = params.as_object() else {
        return Err(crate::session::bad("expected an object"));
    };
    // The map as it is, or the first call's: the target must then come with it.
    let mut map = match &project.view.sun_map {
        Some(m) => serde_json::to_value(m).expect("a sun map serialises"),
        None if given.contains_key("shape") || given.contains_key("outline") => json!({}),
        None => {
            return Err(crate::session::bad(
                "no sun map is on yet: give `shape` or `outline`",
            ));
        }
    };
    let fields = map.as_object_mut().expect("a sun map is an object");
    for (k, v) in given {
        if ![
            "shape", "outline", "date", "season", "years", "cell_m", "opacity",
        ]
        .contains(&k.as_str())
        {
            return Err(crate::session::bad(&format!("unknown parameter `{k}`")));
        }
        match k.as_str() {
            "shape" if !v.is_null() => drop(fields.remove("outline")),
            "outline" if !v.is_null() => drop(fields.remove("shape")),
            "date" if !v.is_null() => drop(fields.remove("season")),
            "season" if !v.is_null() => drop(fields.remove("date")),
            _ => {}
        }
        if v.is_null() {
            fields.remove(k);
        } else {
            fields.insert(k.clone(), v.clone());
        }
    }
    let map: crate::SunMap = parse(&map)?;
    map.validate(project).map_err(|e| crate::session::bad(&e))?;
    project.view.sun_map = Some(map);
    Ok(Value::Null)
}

fn view_clear_sun_map(project: &mut Project, _params: &Value) -> Result<Value, CommandError> {
    project.view.sun_map = None;
    Ok(Value::Null)
}

#[cfg(test)]
mod sun_map_tests {
    use crate::{Project, Session};
    use serde_json::json;

    fn session() -> Session {
        let mut s = Session::new(Project::default());
        s.run(
            "shape.add_area",
            json!({"name": "Lawn", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]]}),
        )
        .unwrap();
        s
    }

    #[test]
    fn the_sun_map_is_set_changed_cleared_and_undone() {
        let mut s = session();
        // The target comes first.
        assert!(s.run("view.set_sun_map", json!({"opacity": 0.3})).is_err());
        s.run(
            "view.set_sun_map",
            json!({"shape": "S1", "season": "winter", "years": 10}),
        )
        .unwrap();
        let m = s.project().view.sun_map.clone().unwrap();
        assert_eq!(m.shape.as_deref(), Some("S1"));
        assert_eq!((m.cell_m, m.opacity, m.years), (0.5, 0.6, Some(10.0)));
        // Only the fields given change; a date replaces the season; null clears the years.
        s.run(
            "view.set_sun_map",
            json!({"opacity": 0.3, "date": {"month": 6, "day": 21}, "years": null}),
        )
        .unwrap();
        let m = s.project().view.sun_map.clone().unwrap();
        assert_eq!((m.opacity, m.years, m.season), (0.3, None, None));
        assert_eq!(m.date.map(|d| (d.month, d.day)), Some((6, 21)));
        // An outline replaces the shape.
        s.run(
            "view.set_sun_map",
            json!({"outline": [[0, 0], [4, 0], [4, 4]]}),
        )
        .unwrap();
        let m = s.project().view.sun_map.clone().unwrap();
        assert!(m.shape.is_none() && m.outline.is_some());
        // Refused: leaves the map as it is.
        for bad in [
            json!({"shape": "S9"}),
            json!({"cell_m": 0.0}),
            json!({"colour": 1}),
            json!({"season": "monsoon"}),
        ] {
            assert!(s.run("view.set_sun_map", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(s.project().view.sun_map.as_ref(), Some(&m));
        s.run("view.clear_sun_map", json!({})).unwrap();
        assert!(s.project().view.sun_map.is_none());
        assert!(s.undo());
        assert_eq!(s.project().view.sun_map.as_ref(), Some(&m));
        assert!(s.undo());
        assert_ne!(s.project().view.sun_map.as_ref(), Some(&m));
    }

    #[test]
    fn view_set_leaves_the_map_alone_and_files_without_one_are_unchanged() {
        let mut s = session();
        let before = serde_json::to_string(s.project()).unwrap();
        assert!(!before.contains("sun_map"));
        s.run("view.set_sun_map", json!({"shape": "S1"})).unwrap();
        s.run("view.set", json!({"haze": 3.0})).unwrap();
        assert!(s.project().view.sun_map.is_some());
        let json = serde_json::to_string(s.project()).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.view.sun_map, s.project().view.sun_map);
        assert!(s.run("view.set", json!({"sun_map": null})).is_err());
        // An old file reads with no map.
        let old: Project = serde_json::from_str(&before).unwrap();
        assert!(old.view.sun_map.is_none());
    }
}

#[cfg(test)]
mod photo_tests {
    use crate::photo::Focus;
    use crate::{Project, Session};
    use serde_json::json;

    #[test]
    fn photo_settings_change_field_by_field_and_stay_out_of_files_by_default() {
        let mut s = Session::new(Project::default());
        let plain = serde_json::to_value(s.project()).unwrap();
        assert!(plain["view"].get("photo").is_none());
        assert!(plain["view"].get("vignette").is_none());
        s.run(
            "view.set_photo",
            json!({"aperture": 2.8, "focus": "auto", "focus_point": [0.5, 0.7]}),
        )
        .unwrap();
        s.run("view.set_photo", json!({"samples": 128})).unwrap();
        let p = s.project().view.photo;
        assert_eq!(
            (p.samples, p.aperture, p.focus, p.focus_point),
            (128, Some(2.8), Focus::Auto, [0.5, 0.7])
        );
        s.run("view.set_photo", json!({"aperture": null, "focus": 4}))
            .unwrap();
        let p = s.project().view.photo;
        assert_eq!((p.aperture, p.focus), (None, Focus::Metres(4.0)));
        for bad in [
            json!({"samples": 0}),
            json!({"aperture": 0.5}),
            json!({"focus": "near"}),
            json!({"focus_point": [2, 0]}),
            json!({"shutter_s": -1}),
            json!({"iso": 100}),
        ] {
            assert!(s.run("view.set_photo", bad.clone()).is_err(), "{bad}");
        }
        s.run("view.set", json!({"vignette": 0.3, "grain": 0.2}))
            .unwrap();
        assert_eq!(
            (s.project().view.vignette, s.project().view.grain),
            (0.3, 0.2)
        );
        assert!(s.run("view.set", json!({"grain": 1.5})).is_err());
        let back: Project =
            serde_json::from_value(serde_json::to_value(s.project()).unwrap()).unwrap();
        assert_eq!(back.view, s.project().view);
    }
}
