//! Candidate list, polite HTTP, `fetch`, `search` and `add`.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    thread::sleep,
    time::Duration,
};

const PH_API: &str = "https://api.polyhaven.com";
const ACG_API: &str = "https://ambientcg.com/api/v3/assets";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    PolyHaven,
    AmbientCg,
}

#[derive(Clone, Deserialize, Debug)]
pub struct Candidate {
    pub category: String,
    pub name: String,
    pub source: String,
    pub page_url: String,
    #[serde(default)]
    pub author: String,
    pub triangles: Option<u32>,
    pub species: Option<String>,
    /// Materials: brick, plaster, concrete, wood, paving, gravel, roof, stone, tile, metal,
    /// ground, fabric, other.
    #[serde(default)]
    pub kind: Option<String>,
    /// Extra tags for models (e.g. `rock`).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Ids of variants to skip (`<id>_c`), judged bad after looking.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Candidate {
    pub fn source(&self) -> Option<Source> {
        match self.source.as_str() {
            "Poly Haven" => Some(Source::PolyHaven),
            "ambientCG" => Some(Source::AmbientCg),
            _ => None,
        }
    }

    /// The source's own id, exactly as in the page URL.
    pub fn source_id(&self) -> &str {
        self.page_url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
    }

    /// The id used in the library: the source id, lower case.
    pub fn id(&self) -> String {
        self.source_id().to_lowercase()
    }

    pub fn dir(&self, cache: &Path) -> PathBuf {
        let s = if self.source() == Some(Source::PolyHaven) {
            "polyhaven"
        } else {
            "ambientcg"
        };
        cache.join(s).join(self.source_id())
    }

    pub fn is_sky(&self) -> bool {
        self.category == "hdri"
    }

    pub fn is_material(&self) -> bool {
        self.category == "material"
    }
}

/// The Poly Haven and ambientCG entries of a candidate file, optionally only some ids.
pub fn load_candidates(paths: &[PathBuf], only: &[String]) -> Result<Vec<Candidate>> {
    let mut all: Vec<Candidate> = Vec::new();
    for path in paths {
        let more: Vec<Candidate> =
            serde_json::from_slice(&fs::read(path).with_context(|| path.display().to_string())?)?;
        all.extend(more);
    }
    let mut seen = std::collections::HashSet::new();
    Ok(all
        .into_iter()
        .filter(|c| seen.insert(c.page_url.clone()))
        .filter(|c| c.source().is_some())
        .filter(|c| only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(c.source_id())))
        .collect())
}

pub struct Http {
    agent: ureq::Agent,
    delay: Duration,
}

impl Http {
    pub fn new(user_agent: &str, delay_ms: u64) -> Self {
        let agent = ureq::AgentBuilder::new()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(180))
            .build();
        Self {
            agent,
            delay: Duration::from_millis(delay_ms),
        }
    }

    fn get(&self, url: &str) -> Result<ureq::Response> {
        let mut last = None;
        for attempt in 1..=3u64 {
            let r = self.agent.get(url).call();
            sleep(self.delay);
            match r {
                Ok(r) => return Ok(r),
                Err(ureq::Error::Status(code, _)) if code != 429 && code < 500 => {
                    bail!("{url}: HTTP {code}")
                }
                Err(e) => {
                    last = Some(e);
                    sleep(Duration::from_secs(3 * attempt));
                }
            }
        }
        bail!("{url}: {}", last.map(|e| e.to_string()).unwrap_or_default())
    }

    pub fn json(&self, url: &str) -> Result<Value> {
        Ok(serde_json::from_reader(self.get(url)?.into_reader())?)
    }

    /// JSON from the cache file when present, else from the network (and cached).
    fn cached_json(&self, url: &str, path: &Path) -> Result<Value> {
        if let Ok(b) = fs::read(path)
            && let Ok(v) = serde_json::from_slice(&b)
        {
            return Ok(v);
        }
        let v = self.json(url)?;
        fs::write(path, serde_json::to_vec_pretty(&v)?)?;
        Ok(v)
    }

    /// Download unless a file that passes the md5/size check is already there; returns whether
    /// it downloaded. Written to `.part` first, so an interrupted run leaves no half file.
    pub fn download(
        &self,
        url: &str,
        dest: &Path,
        md5: Option<&str>,
        size: Option<u64>,
    ) -> Result<bool> {
        let ok = |b: &[u8]| {
            md5.is_none_or(|m| format!("{:x}", ::md5::compute(b)) == m)
                && size.is_none_or(|s| s == b.len() as u64)
        };
        if fs::read(dest).is_ok_and(|b| !b.is_empty() && ok(&b)) {
            return Ok(false);
        }
        fs::create_dir_all(dest.parent().context("no parent")?)?;
        let mut body = Vec::new();
        self.get(url)?.into_reader().read_to_end(&mut body)?;
        if !ok(&body) {
            bail!("{url}: md5 or size does not match what the API announced");
        }
        let part = dest.with_extension("part");
        fs::write(&part, &body)?;
        fs::rename(part, dest)?;
        Ok(true)
    }
}

fn authors(info: &Value) -> Vec<String> {
    info["authors"]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// What the source's API says about one asset, as the library wants it.
pub struct ApiInfo {
    pub name: String,
    pub author: String,
    pub tags: Vec<String>,
}

pub fn ph_info(cache_dir: &Path) -> Result<ApiInfo> {
    let v: Value = serde_json::from_slice(&fs::read(cache_dir.join("info.json"))?)?;
    let mut tags: Vec<String> = v["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|c| !c.contains(':'))
        .map(str::to_string)
        .collect();
    tags.extend(
        v["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(10)
            .map(str::to_string),
    );
    Ok(ApiInfo {
        name: v["name"].as_str().unwrap_or_default().into(),
        author: authors(&v).join(", "),
        tags,
    })
}

fn mismatch(id: &str, field: &str, cand: &str, api: &str) {
    println!("MISMATCH {id} {field}: candidate list `{cand}`, API `{api}`");
}

pub fn fetch(cands: &[Candidate], cache: &Path, http: &Http) -> Result<()> {
    fs::create_dir_all(cache)?;
    let mut failed = Vec::new();
    let mut done: Vec<Candidate> = Vec::new();
    for c in cands {
        match fetch_one(c, cache, http) {
            Ok(n) => {
                println!("ok {} ({n} downloaded)", c.source_id());
                done.push(c.clone());
            }
            Err(e) => {
                println!("FAILED {}: {e:#}", c.source_id());
                failed.push(c.source_id().to_string());
            }
        }
    }
    // ingest reads this copy, so it needs no list flag; merged so a partial fetch keeps the rest.
    let path = cache.join("candidates.json");
    let mut all: Vec<Value> = fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for c in &done {
        all.retain(|v| v["page_url"] != c.page_url);
        all.push(json!({"category": c.category, "name": c.name, "source": c.source, "page_url": c.page_url,
            "author": c.author, "triangles": c.triangles, "species": c.species,
            "kind": c.kind, "tags": c.tags, "exclude": c.exclude}));
    }
    fs::write(path, serde_json::to_vec_pretty(&all)?)?;
    if failed.is_empty() {
        Ok(())
    } else {
        bail!("failed: {}", failed.join(", "))
    }
}

fn fetch_one(c: &Candidate, cache: &Path, http: &Http) -> Result<usize> {
    let (dir, sid) = (c.dir(cache), c.source_id());
    fs::create_dir_all(&dir)?;
    let mut n = 0;
    if c.source() == Some(Source::AmbientCg) {
        let info = http.cached_json(
            &format!("{ACG_API}?id={sid}&include=title,tags,dimensions,downloads"),
            &dir.join("info.json"),
        )?;
        let a = info["assets"].get(0).context("unknown ambientCG id")?;
        let dl = a["downloads"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|d| d["attributes"] == "1K-JPG" && d["extension"] == "zip")
            .context("no 1K-JPG zip")?;
        let zip = dir.join(format!("{sid}_1K-JPG.zip"));
        n += usize::from(http.download(
            dl["url"].as_str().context("no url")?,
            &zip,
            None,
            dl["size"].as_u64(),
        )?);
        zip::ZipArchive::new(fs::File::open(&zip)?).context("downloaded zip is not readable")?;
        return Ok(n);
    }
    let info = http.cached_json(&format!("{PH_API}/info/{sid}"), &dir.join("info.json"))?;
    let (mut api_authors, mut cand_authors) = (
        authors(&info),
        c.author.split(", ").map(str::to_string).collect::<Vec<_>>(),
    );
    api_authors.sort();
    cand_authors.sort();
    if api_authors != cand_authors {
        mismatch(sid, "author", &c.author, &api_authors.join(", "));
    }
    if let (Some(t), Some(p)) = (c.triangles, info["polycount"].as_u64())
        && u64::from(t) != p
    {
        mismatch(sid, "triangles", &t.to_string(), &p.to_string());
    }
    let files = http.cached_json(&format!("{PH_API}/files/{sid}"), &dir.join("files.json"))?;
    let mut get = |e: &Value, rel: Option<&str>| -> Result<()> {
        let url = e["url"].as_str().context("no url")?;
        let rel = rel.unwrap_or_else(|| url.rsplit('/').next().unwrap_or_default());
        n += usize::from(http.download(
            url,
            &dir.join(rel),
            e["md5"].as_str(),
            e["size"].as_u64(),
        )?);
        Ok(())
    };
    if c.is_sky() {
        get(&files["hdri"]["1k"]["hdr"], None)?;
    } else {
        let g = &files["gltf"]["1k"]["gltf"];
        get(g, None)?;
        for (rel, e) in g["include"].as_object().context("gltf has no include")? {
            get(e, Some(rel))?;
        }
    }
    Ok(n)
}

pub fn search(http: &Http, source: &str, query: &str, kind: Option<&str>) -> Result<()> {
    let q = query.to_lowercase();
    if source == "polyhaven" {
        let v = http.json(&format!("{PH_API}/assets?t={}", kind.unwrap_or("models")))?;
        let mut rows: Vec<_> = v
            .as_object()
            .context("unexpected reply")?
            .iter()
            .filter(|(id, a)| {
                let hay =
                    format!("{id} {} {} {}", a["name"], a["tags"], a["categories"]).to_lowercase();
                hay.contains(&q)
            })
            .collect();
        rows.sort_by_key(|(id, _)| id.to_string());
        for (id, a) in rows {
            let dims: Vec<String> = a["dimensions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_f64)
                .map(|d| format!("{:.2}", d / 1000.0))
                .collect();
            println!(
                "{id}\ttriangles={}\tsize_m=[{}]\t{}",
                a["polycount"],
                dims.join(", "),
                a["name"].as_str().unwrap_or_default()
            );
        }
    } else if source == "ambientcg" {
        let v = http.json(&format!(
            "{ACG_API}?type={}&q={}&limit=25&include=title,dimensions",
            kind.unwrap_or("material"),
            query.replace(' ', "+")
        ))?;
        for a in v["assets"].as_array().into_iter().flatten() {
            let d = &a["dimensions"];
            println!(
                "{}\tsize_cm=[{}, {}]\t{}",
                a["id"].as_str().unwrap_or_default(),
                d["width"],
                d["height"],
                a["title"].as_str().unwrap_or_default()
            );
        }
    } else {
        bail!("source must be polyhaven or ambientcg");
    }
    Ok(())
}

/// Append one entry to a candidates file, filled from the live API.
pub fn add(http: &Http, source: &str, id: &str, category: &str, file: &Path) -> Result<()> {
    let entry = if source == "polyhaven" {
        let v = http.json(&format!("{PH_API}/info/{id}"))?;
        json!({"category": category, "name": v["name"], "source": "Poly Haven", "page_url": format!("https://polyhaven.com/a/{id}"),
            "licence": "CC0-1.0 (https://polyhaven.com/license)", "author": authors(&v).join(", "),
            "triangles": v["polycount"], "species": null, "verified": "added with the add command"})
    } else if source == "ambientcg" {
        let v = http.json(&format!("{ACG_API}?id={id}&include=title"))?;
        let a = v["assets"].get(0).context("unknown ambientCG id")?;
        json!({"category": category, "name": a["title"], "source": "ambientCG", "page_url": format!("https://ambientcg.com/a/{}", a["id"].as_str().unwrap_or(id)),
            "licence": "CC0-1.0 (https://docs.ambientcg.com/license/)", "author": "ambientCG (Lennart Demes)",
            "triangles": null, "species": null, "verified": "added with the add command"})
    } else {
        bail!("source must be polyhaven or ambientcg");
    };
    let mut all: Vec<Value> = if file.exists() {
        serde_json::from_slice(&fs::read(file)?)?
    } else {
        Vec::new()
    };
    if all.iter().any(|e| e["page_url"] == entry["page_url"]) {
        bail!("{id} is already in {}", file.display());
    }
    all.push(entry);
    fs::write(file, serde_json::to_vec_pretty(&all)?)?;
    println!("added {id} to {}", file.display());
    Ok(())
}
