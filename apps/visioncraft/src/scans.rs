//! The library's scanned leaf and bark sets (`flora.json`) for plant textures: which set a
//! species uses, fetched and decoded on demand (the asset store's loaders, the browser's
//! decoder on the web). Always the 1k sets: a leaf stamps at 20 to 500 atlas texels and a bark
//! repeat at 128 to 512, below what a 1k sheet holds, so the 2k tier would cost four times the
//! download and memory for nothing visible. Until a species' scans are in, it is drawn; `key` changes when they arrive, and the plant
//! textures are made again from them.

use crate::assets::{AssetStore, Key, Loaded, State};
use std::collections::HashMap;
use std::sync::Arc;
use visioncraft_flora::scan::{BarkScan, Cutout, LeafScan, Scans};
use visioncraft_flora::{Image, PlantParams};

#[derive(serde::Deserialize)]
struct Index {
    leaf_sets: Vec<LeafSet>,
    bark_sets: Vec<BarkSet>,
    species: HashMap<String, SpeciesSets>,
}

#[derive(serde::Deserialize)]
struct LeafSet {
    id: String,
    maps: LeafMaps,
    leaves: Vec<Cutout>,
}

#[derive(serde::Deserialize)]
struct LeafMaps {
    color: String,
    opacity: String,
    normal: Option<String>,
}

#[derive(serde::Deserialize)]
struct BarkSet {
    id: String,
    size_m: Option<[f32; 2]>,
    maps: BarkMaps,
}

#[derive(serde::Deserialize)]
struct BarkMaps {
    basecolor: String,
    normal: Option<String>,
}

/// `[set id, "genus" | "shape"]`: the species' own genus, or a look-alike.
#[derive(serde::Deserialize)]
struct SpeciesSets {
    leaf: Option<(String, String)>,
    leaf_autumn: Option<String>,
    bark: Option<(String, String)>,
}

/// Photo sets without a recorded size (ambientCG's bark scans) cover about this much.
const BARK_DEFAULT_M: [f32; 2] = [1.0, 1.0];

/// A decoded picture as the flora crate takes it.
fn image(store: &mut AssetStore, path: &str) -> Option<Option<Image>> {
    let key = Key::Thumb(path.to_string());
    match store.state(&key) {
        State::Failed => Some(None),
        State::Ready => match store.take(&key) {
            Some(Loaded::Image(i)) => Some(Some(Image {
                width: i.w,
                height: i.h,
                rgba: i.px,
            })),
            _ => Some(None),
        },
        State::Absent | State::Pending => {
            store.want(key, 0.5);
            None
        }
    }
}

/// A set being fetched: its files, each `None` until decoded (`Some(None)`: missing).
#[derive(Default)]
struct Loading {
    files: Vec<(String, Option<Option<Image>>)>,
}

impl Loading {
    fn new(paths: &[&str]) -> Self {
        Self {
            files: paths.iter().map(|p| (p.to_string(), None)).collect(),
        }
    }

    /// Takes what has arrived; the images once all have (a missing one is `None`).
    fn poll(&mut self, store: &mut AssetStore) -> Option<Vec<Option<Image>>> {
        for (path, got) in &mut self.files {
            if got.is_none() {
                *got = image(store, path);
            }
        }
        if self.files.iter().any(|(_, g)| g.is_none()) {
            return None;
        }
        Some(
            self.files
                .iter_mut()
                .filter_map(|(_, g)| g.take())
                .collect(),
        )
    }
}

/// What a species looks like from the scans: its set ids, the kind of match, and the scans
/// themselves once loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub leaf: Option<(String, String)>,
    pub bark: Option<(String, String)>,
}

#[derive(Default)]
pub struct ScanLib {
    index: Option<Index>,
    asked: bool,
    /// Set id to the scan; `None` when it could not be loaded.
    leaves: HashMap<String, Option<Arc<LeafScan>>>,
    barks: HashMap<String, Option<Arc<BarkScan>>>,
    loading_leaf: HashMap<String, Loading>,
    loading_bark: HashMap<String, Loading>,
    /// Bumps when a set finishes loading.
    pub rev: u64,
    /// Plant ids to the built-in species whose sets they use.
    aliases: HashMap<String, String>,
    /// The index is missing or unreadable: everything is drawn.
    given_up: bool,
}

impl ScanLib {
    pub fn alias(&mut self, id: &str, species: &str) {
        if self.aliases.get(id).is_none_or(|s| s != species) {
            self.aliases.insert(id.to_string(), species.to_string());
        }
    }

    /// Fetches the index once and collects the sets on their way.
    pub fn pump(&mut self, store: &mut AssetStore) {
        if self.index.is_none() {
            let key = Key::Bytes("flora.json".into());
            if !self.asked {
                store.want(key.clone(), 0.0);
                self.asked = true;
            }
            if let Some(Loaded::Bytes(b)) = store.take(&key) {
                self.index = serde_json::from_slice(&b).ok();
                self.given_up = self.index.is_none();
                self.rev += 1;
            } else if store.state(&key) == State::Failed {
                self.given_up = true;
            }
            return;
        }
        let index = self.index.as_ref().expect("checked");
        let keys: Vec<_> = self.loading_leaf.keys().cloned().collect();
        for k in keys {
            let Some(got) = self.loading_leaf.get_mut(&k).and_then(|l| l.poll(store)) else {
                continue;
            };
            self.loading_leaf.remove(&k);
            let set = index.leaf_sets.iter().find(|s| s.id == k);
            let scan = match (set, &got[..]) {
                (Some(s), [Some(c), Some(o), n]) => {
                    LeafScan::new(c, o, n.as_ref(), s.leaves.clone()).map(Arc::new)
                }
                _ => None,
            };
            self.leaves.insert(k, scan);
            self.rev += 1;
        }
        let keys: Vec<_> = self.loading_bark.keys().cloned().collect();
        for k in keys {
            let Some(got) = self.loading_bark.get_mut(&k).and_then(|l| l.poll(store)) else {
                continue;
            };
            self.loading_bark.remove(&k);
            let set = index.bark_sets.iter().find(|s| s.id == k);
            let scan = match (set, &got[..]) {
                (Some(s), [Some(c), n]) => Some(Arc::new(BarkScan {
                    color: c.clone(),
                    normal: n.clone(),
                    size_m: s.size_m.unwrap_or(BARK_DEFAULT_M),
                })),
                _ => None,
            };
            self.barks.insert(k, scan);
            self.rev += 1;
        }
    }

    fn want_leaf(&mut self, id: &str) {
        let k = id.to_string();
        if self.leaves.contains_key(&k) || self.loading_leaf.contains_key(&k) {
            return;
        }
        let Some(s) = self
            .index
            .as_ref()
            .and_then(|i| i.leaf_sets.iter().find(|s| s.id == id))
        else {
            return;
        };
        let mut paths = vec![s.maps.color.clone(), s.maps.opacity.clone()];
        paths.extend(s.maps.normal.clone());
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut l = Loading::new(&refs);
        if s.maps.normal.is_none() {
            l.files.push((String::new(), Some(None)));
        }
        self.loading_leaf.insert(k, l);
    }

    fn want_bark(&mut self, id: &str) {
        let k = id.to_string();
        if self.barks.contains_key(&k) || self.loading_bark.contains_key(&k) {
            return;
        }
        let Some(s) = self
            .index
            .as_ref()
            .and_then(|i| i.bark_sets.iter().find(|s| s.id == id))
        else {
            return;
        };
        let mut paths = vec![s.maps.basecolor.clone()];
        paths.extend(s.maps.normal.clone());
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut l = Loading::new(&refs);
        if s.maps.normal.is_none() {
            l.files.push((String::new(), Some(None)));
        }
        self.loading_bark.insert(k, l);
    }

    /// The sets species `id` uses in `month` (its autumn leaves from September to November),
    /// none when it is drawn.
    pub fn sets(&self, id: &str, month: u8, params: &PlantParams) -> Match {
        let none = Match {
            leaf: None,
            bark: None,
        };
        if params.drawn {
            return none;
        }
        let id = self.aliases.get(id).map_or(id, String::as_str);
        let id = id.strip_prefix("flora:").unwrap_or(id);
        // A species the index does not list (a hedge form) takes its genus' sets.
        let genus = id.split('-').next().unwrap_or(id);
        let Some(s) = self.index.as_ref().and_then(|i| {
            i.species.get(id).or_else(|| {
                let mut same: Vec<_> = i
                    .species
                    .iter()
                    .filter(|(k, _)| k.split('-').next() == Some(genus))
                    .collect();
                same.sort_by(|a, b| a.0.cmp(b.0));
                same.first().map(|(_, v)| *v)
            })
        }) else {
            return none;
        };
        let autumn = (9..=11).contains(&month) && params.deciduous;
        Match {
            leaf: autumn
                .then(|| {
                    let kind = s.leaf.as_ref().map_or("shape", |l| l.1.as_str());
                    s.leaf_autumn.clone().map(|a| (a, kind.to_string()))
                })
                .flatten()
                .or_else(|| s.leaf.clone()),
            // A look-alike bark would lose what the drawn one keeps (a birch's white and
            // black, a cherry's red-brown bands): only the genus' own.
            bark: s.bark.clone().filter(|b| b.1 == "genus"),
        }
    }

    /// The scans for species `id` now (asking for those not loaded yet) and a key naming them:
    /// it changes when more arrive.
    pub fn scans(&mut self, id: &str, month: u8, params: &PlantParams) -> (Scans, String) {
        let m = self.sets(id, month, params);
        let mut out = Scans::default();
        let mut key = String::new();
        if let Some((leaf, how)) = &m.leaf {
            match self.leaves.get(leaf) {
                Some(Some(s)) => {
                    out.leaf = Some(s.clone());
                    out.leaf_hue = how == "genus";
                    key.push_str(leaf);
                }
                Some(None) => {}
                None => self.want_leaf(leaf),
            }
        }
        key.push('|');
        if let Some((bark, _)) = &m.bark {
            match self.barks.get(bark) {
                Some(Some(s)) => {
                    out.bark = Some(s.clone());
                    key.push_str(bark);
                }
                Some(None) => {}
                None => self.want_bark(bark),
            }
        }
        (out, key)
    }

    /// Whether species `id`'s sets are still on their way (the index, or a set it uses): its
    /// first textures wait for them rather than being made twice.
    pub fn arriving(&self, id: &str, month: u8, params: &PlantParams) -> bool {
        if self.given_up || params.drawn {
            return false;
        }
        if self.index.is_none() {
            return true;
        }
        let m = self.sets(id, month, params);
        m.leaf.is_some_and(|l| !self.leaves.contains_key(&l.0))
            || m.bark.is_some_and(|b| !self.barks.contains_key(&b.0))
    }

    /// Sets still on their way.
    pub fn loading(&self) -> usize {
        self.loading_leaf.len() + self.loading_bark.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_parses_and_names_look_alikes() {
        let json = r#"{"version": 1,
            "leaf_sets": [{"id": "l1", "maps": {"color": "flora/leaves/l1/color.jpg",
                "opacity": "flora/leaves/l1/opacity.png", "normal": null, "translucency": null},
                "leaves": [{"rect": [0, 0, 1, 1], "pivot": [0.5, 1], "petiole": true,
                "centre": [0.5, 0.5], "axis": [0, -1], "length": 1, "area": 0.5}]}],
            "bark_sets": [{"id": "b1", "size_m": null, "maps": {"basecolor": "flora/bark/b1/basecolor.jpg",
                "normal": "flora/bark/b1/normal.jpg"}}],
            "species": {"acer-x": {"leaf": ["l1", "genus"], "leaf_autumn": "l2", "bark": ["b1", "shape"]}}}"#;
        let lib = ScanLib {
            index: serde_json::from_str(json).ok(),
            ..Default::default()
        };
        let p = PlantParams::default();
        let m = lib.sets("flora:acer-x", 10, &p);
        assert_eq!(m.leaf, Some(("l2".into(), "genus".into())));
        // Not listed, its genus is: the hedge form of a hornbeam.
        assert_eq!(lib.sets("acer-hedge", 6, &p).leaf.unwrap().0, "l1");
        assert_eq!(lib.sets("acer-x", 6, &p).leaf.unwrap().1, "genus");
        let drawn = PlantParams {
            drawn: true,
            ..PlantParams::default()
        };
        assert_eq!(lib.sets("acer-x", 6, &drawn).leaf, None);
    }
}

#[cfg(test)]
mod library_index {
    #[test]
    fn the_shipped_index_parses_and_every_species_set_exists() {
        let json = include_str!("../../../assets/flora.json");
        let i: super::Index = serde_json::from_str(json).expect("flora.json parses");
        let has_leaf = |id: &str| i.leaf_sets.iter().any(|s| s.id == id);
        let has_bark = |id: &str| i.bark_sets.iter().any(|s| s.id == id);
        for (sp, s) in &i.species {
            assert!(s.leaf.as_ref().is_none_or(|l| has_leaf(&l.0)), "{sp}");
            assert!(s.leaf_autumn.as_deref().is_none_or(has_leaf), "{sp}");
            assert!(s.bark.as_ref().is_none_or(|b| has_bark(&b.0)), "{sp}");
        }
        assert!(i.species.len() > 100);
    }
}
