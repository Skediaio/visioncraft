//! Offline asset ingest for the VisionCraft library: fetch, ingest, catalog, check.

#![allow(clippy::needless_range_loop)] // vector maths reads better indexed

mod emit;
mod flora;
mod glb;
mod ingest;
mod library;
mod net;
mod raster;
mod scene;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Sent on every request to the asset APIs; keep it free of personal data.
    #[arg(long, global = true, default_value = "asset-ingest/0.1")]
    user_agent: String,
    /// Pause after every request, in milliseconds.
    #[arg(long, global = true, default_value_t = 500)]
    delay_ms: u64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download the sources of the candidates into the cache (verified, resumable, polite).
    Fetch {
        /// One or more candidate files.
        #[arg(long, num_args = 1.., required_unless_present = "flora")]
        list: Vec<PathBuf>,
        /// Fetch the scanned leaf and bark sets of a `candidates/flora.json` instead.
        #[arg(long)]
        flora: Option<PathBuf>,
        /// Only these source ids.
        #[arg(long, num_args = 1..)]
        only: Vec<String>,
        #[arg(long, default_value = "assets/.cache")]
        cache: PathBuf,
        /// Fetch a higher tier instead of the base set: `2k` (materials, models) or `4k`
        /// (skies, and materials whose candidate says `max_tier: 4k`).
        #[arg(long)]
        tier: Option<String>,
    },
    /// Normalise the cached sources into the library folder.
    Ingest {
        #[arg(long, default_value = "assets/.cache")]
        cache: PathBuf,
        #[arg(long, default_value = "assets")]
        out: PathBuf,
        #[arg(long, num_args = 1..)]
        only: Vec<String>,
        /// Parallel workers.
        #[arg(short, long, default_value_t = 3)]
        jobs: usize,
        /// Build a higher tier next to the base set (see `fetch --tier`).
        #[arg(long)]
        tier: Option<String>,
        /// Ingest the scanned leaf and bark sets of a `candidates/flora.json` instead.
        #[arg(long)]
        flora: Option<PathBuf>,
    },
    /// Write catalog.json, materials.json, skies.json and ATTRIBUTION.md.
    Catalog {
        #[arg(long, default_value = "assets")]
        out: PathBuf,
    },
    /// Validate the library on disk (for CI); non-zero exit on any problem.
    Check {
        #[arg(long, default_value = "assets")]
        out: PathBuf,
    },
    /// Search a source's live API.
    Search {
        #[arg(long)]
        source: String,
        #[arg(long)]
        query: String,
        /// Poly Haven: models|textures|hdris. ambientCG: material|hdri|model|...
        #[arg(long = "type")]
        kind: Option<String>,
    },
    /// Append one entry to a candidates file.
    Add {
        #[arg(long)]
        source: String,
        #[arg(long)]
        id: String,
        #[arg(long)]
        category: String,
        #[arg(long, default_value = "candidates.json")]
        list: PathBuf,
    },
    /// Contact sheet of all model thumbnails.
    Sheet {
        #[arg(long, default_value = "assets")]
        out: PathBuf,
        #[arg(long)]
        to: PathBuf,
    },
    /// Render every light fixture with a marker at its emitter offset, one PNG each.
    Lightsheet {
        #[arg(long, default_value = "assets")]
        out: PathBuf,
        /// Directory for the PNGs.
        #[arg(long)]
        to: PathBuf,
    },
    /// Render the LODs of one asset side by side.
    Lodcmp {
        #[arg(long, default_value = "assets")]
        out: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        to: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let http = net::Http::new(&cli.user_agent, cli.delay_ms);
    match cli.cmd {
        Cmd::Fetch {
            list,
            flora,
            only,
            cache,
            tier,
        } => {
            let cands = match flora {
                Some(f) => flora::FloraSpec::load(&f)?.candidates(),
                None => net::load_candidates(&list, &only)?,
            };
            net::fetch(&cands, &cache, &http, tier.as_deref())
        }
        Cmd::Ingest {
            cache,
            out,
            only,
            jobs,
            tier,
            flora,
        } => {
            if let Some(f) = flora {
                return flora::ingest_all(
                    &flora::FloraSpec::load(&f)?,
                    &cache,
                    &out,
                    tier.as_deref(),
                );
            }
            let cands = net::load_candidates(&[cache.join("candidates.json")], &only)?;
            ingest::ingest_all(&cands, &cache, &out, jobs, tier.as_deref())
        }
        Cmd::Catalog { out } => library::catalog(&out),
        Cmd::Check { out } => {
            let errors = library::check(&out);
            for e in &errors {
                println!("ERROR {e}");
            }
            if errors.is_empty() {
                println!("check: ok");
                Ok(())
            } else {
                bail!("{} problem(s)", errors.len())
            }
        }
        Cmd::Search {
            source,
            query,
            kind,
        } => net::search(&http, &source, &query, kind.as_deref()),
        Cmd::Add {
            source,
            id,
            category,
            list,
        } => net::add(&http, &source, &id, &category, &list),
        Cmd::Sheet { out, to } => library::sheet(&out, &to),
        Cmd::Lightsheet { out, to } => library::lightsheet(&out, &to),
        Cmd::Lodcmp { out, id, to } => library::lodcmp(&out, &id, &to),
    }
}

#[cfg(test)]
mod tests;
