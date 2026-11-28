//! `release-notes`: prints the notes of a release as Markdown (see `release_notes`).

use std::{
    env, fs,
    path::Path,
    process::{self, Command},
};

use release_notes::{Commit, Pin, manifest_paths, notes, parse_log, pins, previous_tag};

const USAGE: &str = "\
release-notes [--previous TAG] [REF]

Prints the notes of REF, a tag or any commit (the default is HEAD), as Markdown: the
Conventional Commits since the previous release tag, and the git tags the workspace pins.
Run it inside a checkout of the repository.

  --previous TAG   The tag the notes start after. By default the highest release tag
                   reachable from REF; a pre-release tag is not one, so its notes cover
                   the whole cycle since the release before it.";

fn main() {
    if let Err(error) = run() {
        eprintln!("release-notes: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut previous = None;
    let mut reference = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--previous" => {
                previous = Some(
                    args.next()
                        .ok_or_else(|| "--previous needs a tag".to_owned())?
                        .clone(),
                )
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option {option}\n\n{USAGE}"));
            }
            other => reference = Some(other.to_owned()),
        }
    }

    let root = git(&["rev-parse", "--show-toplevel"])?;
    env::set_current_dir(root.trim())
        .map_err(|error| format!("cannot enter the checkout: {error}"))?;

    let reference = reference.as_deref().unwrap_or("HEAD");
    let commit = git(&["rev-parse", "--verify", &format!("{reference}^{{commit}}")])?;
    let commit = commit.trim();
    // REF is the tag being released when a tag of that name exists: it is not its own predecessor.
    let is_tag = git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/tags/{reference}"),
    ])
    .is_ok();
    let tags = git(&["tag", "--merged", commit, "--list", "v*"])?;
    let previous = match previous {
        Some(previous) => Some(previous),
        None => previous_tag(tags.lines(), if is_tag { reference } else { "" }),
    };
    let range = previous.as_deref().map_or_else(
        || commit.to_owned(),
        |previous| format!("{previous}..{commit}"),
    );

    let log = git(&["log", "--no-merges", "--format=%s%x1f%b%x1e", &range])?;
    let commits: Vec<Commit> = parse_log(&log);
    let manifests = manifest_paths(Path::new("."));
    let found: Vec<Pin> = pins(
        manifests
            .iter()
            .filter_map(|path| fs::read_to_string(path).ok()),
    );
    print!("{}", notes(&commits, previous.as_deref(), &found));
    Ok(())
}

/// Runs git here and returns what it printed; its complaint when it fails.
fn git(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("cannot start git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
