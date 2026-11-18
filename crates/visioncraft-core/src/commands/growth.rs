//! Growth over the years: the sizes a planting reaches (`planting.growth`) and where crowns
//! will meet walls or each other (`planting.growth_conflicts`). Read-only.

use super::fail;
use crate::growth::{Grower, growth_of};
use crate::planting::Generator;
use crate::project::{Project, YEARS_MAX};
use crate::session::{CommandError, Session, bad, parse};
use crate::shape::Shape;
use crate::spec::{ParamType as T, param, spec};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn register(s: &mut Session) {
    s.register_with_spec(
        spec(
            "planting.growth",
            "Growth of a Planting",
            "How the plants of a planting grow: per asset its planted age, years to maturity and, for every year from planting to `years`, height, width and trunk diameter. From one table of estimated growth data per species (marked `estimated`); assets not in it are guessed from their size.",
            vec![
                param("id", T::String, "Planting id.").required(),
                param("years", T::Number, "Years after planting to go to (0..60).").default(json!(30)),
            ],
            "{id, assets: [{asset, grower, estimated, planted_age_years, years_to_maturity, mature_height_m, mature_width_m, spacing_m, years: [{year, height_m, width_m, trunk_diameter_m}]}]}",
            json!({"id": "g2", "years": 20}),
        )
        .read_only(),
        planting_growth,
    );
    s.register_with_spec(
        spec(
            "planting.canopy_cover",
            "Canopy Cover of a Planting",
            "How much of a planted area (an area scatter or a brush) its woody plants close from above: the share of the area under at least one crown, counted on a grid, from the placements and each plant's crown width (growth table width at its age, times its scale). Trees, conifers, palms and shrubs count. `years` after planting shows how it closes as the plants grow.",
            vec![
                param("id", T::String, "Planting id (an area scatter or a brush).").required(),
                param("years", T::Number, "Years after planting (0..60).").default(json!(0)),
                param("cell_m", T::Number, "Grid cell (m, 0.1..10).").default(json!(1)),
            ],
            "{id, years, area_m2, covered_m2, cover (0..1), crowns, mean_crown_width_m, cell_m}",
            json!({"id": "g1", "years": 30}),
        )
        .read_only(),
        canopy_cover,
    );
    s.register_with_spec(
        spec(
            "planting.growth_conflicts",
            "Growth Conflicts",
            "Where trees (and single shrubs) will grow into a building or into each other: the first year (up to `years`) a crown reaches a block's wall or two crowns meet. Trees of lines and scatters count too, at most 400 plants.",
            vec![param("years", T::Number, "How far to look ahead (0..60).").default(json!(30))],
            "{conflicts: [{planting, name, with, kind (wall|crown), year}]}",
            json!({"years": 20}),
        )
        .read_only(),
        growth_conflicts,
    );
}

/// The most plants `planting.growth_conflicts` compares.
const MAX_PLANTS: usize = 400;

/// The age (years) a planting's plants of `asset` have when planted.
pub fn planted_age(g: &Generator, asset: &str) -> f32 {
    g.planted_age_years()
        .unwrap_or_else(|| growth_of(asset, None).planted_age_years)
}

fn planting_growth(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        #[serde(default = "thirty")]
        years: f32,
    }
    let p: P = parse(params)?;
    if !(0.0..=YEARS_MAX).contains(&p.years) {
        return Err(bad("years: 0..60"));
    }
    let g = project
        .generators
        .iter()
        .find(|g| g.id() == p.id)
        .ok_or_else(|| fail(format!("no planting `{}`", p.id)))?;
    let assets: Vec<Value> = g
        .assets()
        .into_iter()
        .map(|asset| {
            let gr = growth_of(asset, None);
            let age0 = planted_age(g, asset);
            let years: Vec<Value> = (0..=p.years.floor() as u32)
                .map(|y| {
                    let age = age0 + y as f32;
                    let s = gr.size(age);
                    json!({
                        "year": y,
                        "height_m": round(gr.mature_height_m * s.height),
                        "width_m": round(gr.mature_width_m * s.width),
                        "trunk_diameter_m": round(gr.trunk_diameter_m(age)),
                    })
                })
                .collect();
            json!({
                "asset": asset,
                "grower": gr.grower,
                "estimated": true,
                "from_table": gr.from_table,
                "planted_age_years": age0,
                "years_to_maturity": gr.years_to_maturity,
                "mature_height_m": gr.mature_height_m,
                "mature_width_m": gr.mature_width_m,
                // Plant at a spacing that closes once grown: a hedge's plants touch at its
                // width, border plants a little closer than their full width.
                "spacing_m": round(gr.mature_width_m * if gr.grower == Grower::Clipped { 0.6 } else { 0.8 }),
                "years": years,
            })
        })
        .collect();
    Ok(json!({"id": p.id, "assets": assets}))
}

/// The crowns (centre, radius) of a generator's woody plants `years` after planting: trees,
/// conifers and palms, and with `shrubs` the shrubs too.
pub(super) fn crowns(
    project: &Project,
    g: &Generator,
    years: f32,
    shrubs: bool,
) -> Vec<([f32; 2], f32)> {
    let floor = crate::terrace::ground_fn(project);
    let ground = |x: f32, z: f32| floor(x, z).or(Some(0.0));
    crate::planting::evaluate(g, &ground)
        .placements
        .iter()
        .filter_map(|pl| {
            let gr = growth_of(&pl.asset, None);
            (matches!(gr.grower, Grower::Tree | Grower::Conifer | Grower::Palm)
                || (shrubs && gr.grower == Grower::Shrub))
                .then(|| {
                    let age = planted_age(g, &pl.asset) + years;
                    let width = gr.mature_width_m * gr.size(age).width * pl.scale;
                    ([pl.position[0], pl.position[2]], width / 2.0)
                })
        })
        .collect()
}

fn canopy_cover(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        id: String,
        #[serde(default)]
        years: f32,
        #[serde(default = "one")]
        cell_m: f32,
    }
    let p: P = parse(params)?;
    if !(0.0..=YEARS_MAX).contains(&p.years) {
        return Err(bad("years: 0..60"));
    }
    if !(0.1..=10.0).contains(&p.cell_m) {
        return Err(bad("cell_m: 0.1..10"));
    }
    let g = project
        .generators
        .iter()
        .find(|g| g.id() == p.id)
        .ok_or_else(|| fail(format!("no planting `{}`", p.id)))?;
    let (bounds, inside) = crate::planting::planted_area(g)
        .ok_or_else(|| fail("canopy cover needs an area scatter or a brush"))?;
    let crowns = crowns(project, g, p.years, true);
    let c = crate::canopy::canopy_cover(bounds, &*inside, &crowns, p.cell_m);
    let mean = crowns.iter().map(|c| 2.0 * c.1).sum::<f32>() / crowns.len().max(1) as f32;
    Ok(json!({
        "id": p.id,
        "years": p.years,
        "area_m2": round(c.area_m2),
        "covered_m2": round(c.covered_m2),
        "cover": round(c.fraction),
        "crowns": crowns.len(),
        "mean_crown_width_m": round(mean),
        "cell_m": c.cell_m,
    }))
}

fn one() -> f32 {
    1.0
}

fn thirty() -> f32 {
    30.0
}

fn round(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn growth_conflicts(project: &mut Project, params: &Value) -> Result<Value, CommandError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct P {
        #[serde(default = "thirty")]
        years: f32,
    }
    let p: P = parse(params)?;
    if !(0.0..=YEARS_MAX).contains(&p.years) {
        return Err(bad("years: 0..60"));
    }
    // Trees and shrubs of every planting (single ones and those of lines and scatters, by
    // where they are placed): (planting id, name, x, z, growth, planted age, scale). Scattered
    // plantings count their trees only, and at most `MAX_PLANTS` plants are compared.
    let mut plants = Vec::new();
    let floor = crate::terrace::ground_fn(project);
    for g in &project.generators {
        if matches!(g, Generator::SurfaceFill(_)) {
            continue; // placed on a model's faces: the app knows those, the core does not
        }
        let single = matches!(g, Generator::Single(_));
        let ground = |x: f32, z: f32| floor(x, z).or(Some(0.0));
        for (k, pl) in crate::planting::evaluate(g, &ground)
            .placements
            .iter()
            .enumerate()
        {
            let gr = growth_of(&pl.asset, None);
            let woody = if single {
                matches!(
                    gr.grower,
                    Grower::Tree | Grower::Conifer | Grower::Palm | Grower::Shrub
                )
            } else {
                matches!(gr.grower, Grower::Tree | Grower::Conifer | Grower::Palm)
            };
            if woody && plants.len() < MAX_PLANTS {
                let name = if single {
                    g.name().to_string()
                } else {
                    format!("{} (#{})", g.name(), k + 1)
                };
                plants.push((
                    g.id().to_string(),
                    name,
                    [pl.position[0], pl.position[2]],
                    gr,
                    planted_age(g, &pl.asset),
                    pl.scale,
                ));
            }
        }
    }
    type Block<'a> = (&'a str, &'a str, std::borrow::Cow<'a, [[f32; 2]]>);
    let blocks: Vec<Block> = project
        .shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Extrusion(e) if e.closed => Some((e.id.as_str(), e.name.as_str(), e.outline())),
            _ => None,
        })
        .collect();
    let radius = |gr: &crate::growth::Growth, age: f32, scale: f32| {
        gr.mature_width_m * gr.size(age).width * scale / 2.0
    };
    let mut out = Vec::new();
    for (i, (id, name, at, gr, age0, scale)) in plants.iter().enumerate() {
        for (bid, bname, outline) in &blocks {
            let d = distance_to_outline(*at, outline);
            if let Some(y) =
                (0..=p.years as u32).find(|y| radius(gr, age0 + *y as f32, *scale) >= d)
            {
                out.push(json!({"planting": id, "name": name, "with": bid, "with_name": bname, "kind": "wall", "year": y}));
            }
        }
        for (oid, oname, oat, ogr, oage0, oscale) in plants.iter().skip(i + 1) {
            let d = ((at[0] - oat[0]).powi(2) + (at[1] - oat[1]).powi(2)).sqrt();
            if let Some(y) = (0..=p.years as u32).find(|y| {
                radius(gr, age0 + *y as f32, *scale) + radius(ogr, oage0 + *y as f32, *oscale) >= d
            }) {
                out.push(json!({"planting": id, "name": name, "with": oid, "with_name": oname, "kind": "crown", "year": y}));
            }
        }
    }
    out.sort_by_key(|c| c["year"].as_u64());
    Ok(json!({ "conflicts": out }))
}

/// Distance from a point to a closed outline's edges (0 inside it).
fn distance_to_outline(p: [f32; 2], outline: &[[f32; 2]]) -> f32 {
    if crate::shape::inside_outline(p, outline) {
        return 0.0;
    }
    (0..outline.len())
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
            let (ab, ap) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
            let len2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-12);
            let u = ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0);
            (ap[0] - ab[0] * u).hypot(ap[1] - ab[1] * u)
        })
        .fold(f32::INFINITY, f32::min)
}

#[cfg(test)]
mod tests {
    use crate::{Project, Session};
    use serde_json::json;

    #[test]
    fn canopy_cover_rises_with_the_years_and_needs_an_area() {
        let mut s = Session::new(Project::default());
        s.run(
            "planting.add_area",
            json!({"name": "Wood", "outline": [[0, 0], [40, 0], [40, 40], [0, 40]], "seed": 2,
                "asset_palette": [{"asset": "flora:fagus-sylvatica", "weight": 1}],
                "density_per_m2": 0.02}),
        )
        .unwrap();
        let cover = |s: &mut Session, years: u32| {
            s.run("planting.canopy_cover", json!({"id": "g1", "years": years}))
                .unwrap()
        };
        let (young, old) = (cover(&mut s, 0), cover(&mut s, 50));
        assert_eq!(young["area_m2"], 1600.0);
        assert!(
            young["cover"].as_f64() < old["cover"].as_f64(),
            "{young} {old}"
        );
        assert!(old["cover"].as_f64().unwrap() > 0.7, "{old}");
        s.run(
            "planting.add_single",
            json!({"name": "Oak", "asset": "flora:quercus-robur", "position": [5, 0, 5]}),
        )
        .unwrap();
        assert!(s.run("planting.canopy_cover", json!({"id": "g2"})).is_err());
    }

    #[test]
    fn a_tree_grows_and_reaches_the_house() {
        let mut s = Session::new(Project::default());
        s.run(
            "shape.add_extrusion",
            json!({"name": "House", "outline": [[0, 0], [10, 0], [10, 8], [0, 8]], "height": 6}),
        )
        .unwrap();
        s.run(
            "planting.add_single",
            json!({"name": "Maple", "asset": "flora:acer-campestre", "position": [5, 0, 11]}),
        )
        .unwrap();
        let g = s
            .run("planting.growth", json!({"id": "g1", "years": 20}))
            .unwrap();
        let years = g["assets"][0]["years"].as_array().unwrap();
        assert_eq!(years.len(), 21);
        let h = |y: usize| years[y]["height_m"].as_f64().unwrap();
        assert!(h(0) > 1.0 && h(20) > h(10) && h(10) > h(0), "{years:?}");
        assert_eq!(g["assets"][0]["planted_age_years"], 8.0);
        // A planting age given by the user.
        s.run(
            "planting.update",
            json!({"id": "g1", "planted_age_years": 2}),
        )
        .unwrap();
        let g2 = s
            .run("planting.growth", json!({"id": "g1", "years": 20}))
            .unwrap();
        assert!(g2["assets"][0]["years"][0]["height_m"].as_f64() < years[0]["height_m"].as_f64());
        // 3 m from the wall: a field maple (8 m wide) meets it some years on.
        let c = s
            .run("planting.growth_conflicts", json!({"years": 30}))
            .unwrap();
        let wall = &c["conflicts"][0];
        assert_eq!(wall["kind"], "wall");
        assert!(wall["year"].as_u64().unwrap() > 0, "{c}");
        // Growing is a view: the project's years.
        s.run("view.set", json!({"years": 10})).unwrap();
        assert_eq!(s.project().view.years, Some(10.0));
        assert!(s.run("view.set", json!({"years": 99})).is_err());
        s.run("view.set", json!({"years": null})).unwrap();
        assert_eq!(s.project().view.years, None);
    }
}
