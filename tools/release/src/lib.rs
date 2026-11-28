//! The notes of a release: the Conventional Commits since the previous release tag, and the
//! git tags the workspace pins. Text only, so the tests need no repository; `main.rs` runs git.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use Section::{Breaking, Features, Fixes, Other, Performance};

/// One commit, as `git log` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub subject: String,
    pub body: String,
}

/// The part of the notes a commit belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Breaking,
    Features,
    Fixes,
    Performance,
    Other,
}

/// A Conventional Commit subject: `type(scope)!: description`.
struct Conventional<'a> {
    kind: &'a str,
    scope: Option<&'a str>,
    bang: bool,
    description: &'a str,
}

/// Parses `type(scope)!: description`. The type is lowercase letters only, so a subject such as
/// `AGENTS.md: ...` or `Curved houses: ...` is not taken for one.
fn conventional(subject: &str) -> Option<Conventional<'_>> {
    let (head, description) = subject.split_once(": ")?;
    let (head, bang) = match head.strip_suffix('!') {
        Some(head) => (head, true),
        None => (head, false),
    };
    let (kind, scope) = match head.split_once('(') {
        Some((kind, scope)) => (kind, Some(scope.strip_suffix(')')?)),
        None => (head, None),
    };
    if kind.is_empty() || !kind.bytes().all(|b| b.is_ascii_lowercase()) || description.is_empty() {
        return None;
    }
    Some(Conventional {
        kind,
        scope,
        bang,
        description,
    })
}

/// The text of a `BREAKING CHANGE:` (or `BREAKING-CHANGE:`) footer, up to the end of its
/// paragraph.
fn breaking_change(body: &str) -> Option<String> {
    const TOKENS: [&str; 2] = ["BREAKING CHANGE: ", "BREAKING-CHANGE: "];
    let mut lines = body.lines();
    let mut text = None;
    for line in lines.by_ref() {
        if let Some(rest) = TOKENS.iter().find_map(|token| line.strip_prefix(token)) {
            text = Some(rest.trim().to_owned());
            break;
        }
    }
    let mut text = text?;
    for line in lines {
        if line.trim().is_empty() {
            break;
        }
        text.push(' ');
        text.push_str(line.trim());
    }
    Some(text)
}

/// The part of the notes a commit belongs to. `!` or a `BREAKING CHANGE` footer makes it a
/// breaking change, whatever its type.
pub fn section(commit: &Commit) -> Section {
    if breaking_change(&commit.body).is_some() {
        return Breaking;
    }
    match conventional(&commit.subject) {
        Some(conventional) if conventional.bang => Breaking,
        Some(conventional) => match conventional.kind {
            "feat" => Features,
            "fix" => Fixes,
            "perf" => Performance,
            _ => Other,
        },
        None => Other,
    }
}

/// The line of a commit in its section. A breaking change says why.
pub fn bullet(commit: &Commit) -> String {
    let mut text = match conventional(&commit.subject) {
        // A commit of another type keeps its type in front: that is what tells the story.
        Some(conventional) if section(commit) != Other => match conventional.scope {
            Some(scope) => format!("- **{scope}:** {}", conventional.description),
            None => format!("- {}", conventional.description),
        },
        _ => format!("- {}", commit.subject),
    };
    if let Some(why) = breaking_change(&commit.body) {
        text.push_str(" — ");
        text.push_str(&why);
    }
    text
}

/// Splits the output of `git log --format=%s%x1f%b%x1e` into commits, newest first.
pub fn parse_log(log: &str) -> Vec<Commit> {
    log.split('\x1e')
        .filter(|record| !record.trim().is_empty())
        .map(|record| {
            let mut fields = record.split('\x1f');
            Commit {
                subject: fields.next().unwrap_or_default().trim().to_owned(),
                body: fields.next().unwrap_or_default().trim().to_owned(),
            }
        })
        .collect()
}

/// The numbers of a version tag: `v0.1.3` and `v0.1.3-rc.1` are both `(0, 1, 3)`. Anything
/// else than three numbers makes it None.
fn version_numbers(tag: &str) -> Option<(u64, u64, u64)> {
    let version = tag.strip_prefix('v')?;
    let version = version.split(['-', '+']).next()?;
    let mut numbers = version.split('.').map(|number| number.parse::<u64>());
    let key = (
        numbers.next()?.ok()?,
        numbers.next()?.ok()?,
        numbers.next()?.ok()?,
    );
    numbers.next().is_none().then_some(key)
}

/// Whether a tag is a release (`vX.Y.Z`), not a pre-release such as `v0.1.0-rc.1`.
fn is_release(tag: &str) -> bool {
    tag.strip_prefix('v')
        .is_some_and(|version| !version.contains(['-', '+']))
}

/// The highest release tag (`vX.Y.Z`) among `tags` that is not `current` and is lower than it.
/// A pre-release is not a release: the notes of the next tag, and of a pre-release itself,
/// cover the whole cycle since the release before it.
pub fn previous_tag<'a>(tags: impl IntoIterator<Item = &'a str>, current: &str) -> Option<String> {
    let current_version = version_numbers(current);
    tags.into_iter()
        .filter(|tag| *tag != current && is_release(tag))
        .filter_map(|tag| Some((version_numbers(tag)?, tag)))
        .filter(|(version, _)| current_version.is_none_or(|current| *version < current))
        .max_by_key(|(version, _)| *version)
        .map(|(_, tag)| tag.to_owned())
}

/// A git repository the workspace depends on, and how it is pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// The repository's name: the last path segment of its URL, without `.git`.
    pub name: String,
    pub url: String,
    /// What the manifests pin it to, such as `tag v0.1.3` or `rev 58bfc60`.
    pub refs: Vec<String>,
}

/// The `git` dependencies of the manifests and what they are pinned to: one `Pin` per
/// repository, by name.
pub fn pins(manifests: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<Pin> {
    let mut by_url: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for manifest in manifests {
        for url in without_comments(manifest.as_ref()).split('{').skip(1) {
            let Some(table) = url.split_once('}').map(|(table, _)| table) else {
                continue;
            };
            if let Some(url) = field(table, "git") {
                let pinned = ["tag", "rev", "branch"]
                    .iter()
                    .find_map(|key| Some(format!("{key} {}", field(table, key)?)))
                    .unwrap_or_else(|| "unpinned".to_owned());
                by_url.entry(url.to_owned()).or_default().insert(pinned);
            }
        }
    }
    let mut pins: Vec<Pin> = by_url
        .into_iter()
        .map(|(url, refs)| Pin {
            name: repository_name(&url).to_owned(),
            url: url.clone(),
            refs: refs.into_iter().collect(),
        })
        .collect();
    pins.sort_by(|a, b| (&a.name, &a.url).cmp(&(&b.name, &b.url)));
    pins
}

/// The name of a git repository: the last path segment of its URL.
fn repository_name(url: &str) -> &str {
    let path = url.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    path.rsplit('/').next().unwrap_or(path)
}

/// The value of a string key of an inline table: `key = "value"`.
fn field<'a>(table: &'a str, key: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(at) = table[from..].find(key) {
        let start = from + at;
        from = start + key.len();
        let stands_alone = start == 0 || table[..start].ends_with([' ', '\t', '\n', ',']);
        let Some(value) = table[from..].trim_start().strip_prefix("= \"") else {
            continue;
        };
        if let (Some(end), true) = (value.find('"'), stands_alone) {
            return Some(&value[..end]);
        }
    }
    None
}

/// A manifest without its comments: of every line, everything before a `#` outside a string.
fn without_comments(manifest: &str) -> String {
    manifest
        .lines()
        .map(|line| {
            let end = line
                .match_indices('#')
                .find(|(at, _)| line[..*at].matches('"').count() % 2 == 0)
                .map_or(line.len(), |(at, _)| at);
            &line[..end]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `Cargo.toml` under `root`, in a stable order. Hidden directories (`.git`, editor and
/// harness settings) and build output (`target`, `node_modules`) are left out.
pub fn manifest_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    walk(root, &mut paths);
    paths.sort();
    paths
}

fn walk(dir: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                walk(&path, paths);
            }
        } else if name == "Cargo.toml" {
            paths.push(path);
        }
    }
}

const ASSET_LIBRARY: &str = "\
The asset library is not in an archive. Build it from a source checkout with
`tools/visioncraft-assets` (see \"Assets\" in the README); the desktop application takes it with
`--assets <dir>`, and the web build loads it from `assets/` beside the page.";

/// The notes as Markdown: breaking changes first, then features, fixes and performance; the
/// other changes folded away; then the pins and how to get the asset library.
pub fn notes(commits: &[Commit], previous: Option<&str>, pins: &[Pin]) -> String {
    let mut text = match (previous, commits.is_empty()) {
        (None, _) => "Every change up to the first release.\n".to_owned(),
        (Some(previous), true) => format!("Nothing changed since `{previous}`.\n"),
        (Some(previous), false) => format!("Changes since `{previous}`.\n"),
    };
    let bullets = |part| {
        commits
            .iter()
            .filter(|commit| section(commit) == part)
            .map(bullet)
            .collect::<Vec<_>>()
    };
    for (part, title) in [
        (Breaking, "Breaking changes"),
        (Features, "Features"),
        (Fixes, "Fixes"),
        (Performance, "Performance"),
    ] {
        let lines = bullets(part);
        if lines.is_empty() {
            continue;
        }
        text.push_str(&format!("\n### {title}\n"));
        text.extend(lines.into_iter().map(|line| format!("{line}\n")));
    }
    let other = bullets(Other);
    if !other.is_empty() {
        text.push_str(&format!(
            "\n<details>\n<summary>Other changes ({})</summary>\n\n",
            other.len()
        ));
        text.extend(other.into_iter().map(|line| format!("{line}\n")));
        text.push_str("</details>\n");
    }
    if !pins.is_empty() {
        text.push_str("\n### Pins\n\n");
        for pin in pins {
            let refs: Vec<String> = pin
                .refs
                .iter()
                .map(|pinned| match pinned.split_once(' ') {
                    Some((kind, value)) => format!("{kind} `{value}`"),
                    None => format!("`{pinned}`"),
                })
                .collect();
            text.push_str(&format!(
                "- {}: {} ({})\n",
                pin.name,
                refs.join(", "),
                pin.url
            ));
        }
    }
    text.push_str("\n### Asset library\n\n");
    text.push_str(ASSET_LIBRARY);
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(subject: &str, body: &str) -> Commit {
        Commit {
            subject: subject.into(),
            body: body.into(),
        }
    }

    fn headings(notes: &str) -> Vec<&str> {
        notes
            .lines()
            .filter(|line| line.starts_with("### "))
            .collect()
    }

    #[test]
    fn a_commit_goes_to_the_section_of_its_type() {
        assert_eq!(
            section(&commit(
                "feat(render): relight the sky off the UI thread",
                ""
            )),
            Section::Features
        );
        assert_eq!(
            section(&commit("fix: repair the plan camera", "")),
            Section::Fixes
        );
        assert_eq!(
            section(&commit("perf(shadows): spread the cascade redraws", "")),
            Section::Performance
        );
        assert_eq!(
            section(&commit("feat(cli)!: drop the --bridge flag", "")),
            Section::Breaking
        );
        assert_eq!(
            section(&commit(
                "fix(cli): repair the flags",
                "Closes the gap.\n\nBREAKING CHANGE: the CLI needs Node 22.\n"
            )),
            Section::Breaking
        );
        assert_eq!(
            section(&commit("docs: how to cut a release", "")),
            Section::Other
        );
        assert_eq!(
            section(&commit("Curved houses: houses build on arcs", "")),
            Section::Other
        );
        assert_eq!(
            section(&commit("AGENTS.md: how to use the build server", "")),
            Section::Other
        );
    }

    #[test]
    fn a_bullet_shows_the_scope_in_bold() {
        assert_eq!(
            bullet(&commit("feat(render): relight the sky", "")),
            "- **render:** relight the sky"
        );
        assert_eq!(
            bullet(&commit("fix: repair the plan camera", "")),
            "- repair the plan camera"
        );
    }

    #[test]
    fn a_bullet_of_another_change_keeps_its_type() {
        assert_eq!(
            bullet(&commit("docs: how to cut a release", "")),
            "- docs: how to cut a release"
        );
        assert_eq!(
            bullet(&commit("ci(release): build on a tag", "")),
            "- ci(release): build on a tag"
        );
        assert_eq!(
            bullet(&commit("Curved houses: houses build on arcs", "")),
            "- Curved houses: houses build on arcs"
        );
    }

    #[test]
    fn a_breaking_change_shows_why() {
        let bang = commit("feat(cli)!: drop --bridge", "");
        assert_eq!(bullet(&bang), "- **cli:** drop --bridge");
        let footer = commit(
            "fix(cli): repair the flags",
            "A longer story.\n\nBREAKING CHANGE: --bridge is gone.\nUse the control channel directly.\n",
        );
        assert_eq!(
            bullet(&footer),
            "- **cli:** repair the flags — --bridge is gone. Use the control channel directly."
        );
    }

    #[test]
    fn the_log_splits_into_commits() {
        let log = "feat: one\x1fbody of one\n\x1e\nfix: two\x1f\x1e\n";
        assert_eq!(
            parse_log(log),
            vec![
                Commit {
                    subject: "feat: one".into(),
                    body: "body of one".into()
                },
                Commit {
                    subject: "fix: two".into(),
                    body: String::new()
                },
            ]
        );
        assert!(parse_log("").is_empty());
    }

    #[test]
    fn the_notes_list_breaking_changes_first_then_feat_fix_perf() {
        let commits = [
            commit("perf(sky): relight it faster", ""),
            commit("Curved houses: houses build on arcs", ""),
            commit("feat(render): relight the sky", ""),
            commit("docs: how to cut a release", ""),
            commit("fix: repair the plan camera", ""),
            commit("feat(cli)!: drop --bridge", ""),
        ];
        let text = notes(&commits, Some("v0.1.0"), &[]);
        assert_eq!(
            headings(&text),
            vec![
                "### Breaking changes",
                "### Features",
                "### Fixes",
                "### Performance",
                // The other changes are folded away below, not a heading of their own.
                "### Asset library",
            ]
        );
        assert!(text.starts_with("Changes since `v0.1.0`.\n"), "{text}");
        assert!(text.contains("\n- **cli:** drop --bridge\n"), "{text}");
        assert!(text.contains("\n- **render:** relight the sky\n"), "{text}");
        assert!(text.contains("\n- repair the plan camera\n"), "{text}");
        assert!(text.contains("\n- **sky:** relight it faster\n"), "{text}");
        assert!(
            text.contains(
                "<details>\n<summary>Other changes (2)</summary>\n\n- Curved houses: houses build on arcs\n- docs: how to cut a release\n</details>\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn the_notes_name_the_previous_tag_or_the_first_release() {
        let commits = [commit("feat: one", "")];
        assert!(notes(&commits, None, &[]).starts_with("Every change up to the first release.\n"));
        assert!(notes(&[], Some("v0.1.0"), &[]).contains("Nothing changed since `v0.1.0`."));
        // An empty section is left out.
        assert_eq!(
            headings(&notes(&commits, None, &[])),
            vec!["### Features", "### Asset library"]
        );
    }

    #[test]
    fn the_previous_tag_is_the_highest_release_before_the_current() {
        let tags = [
            "v0.9.0",
            "v0.10.0-rc.1",
            "v0.10.0",
            "v0.10.1",
            "v1.0.0",
            "wip",
            "v2",
        ];
        assert_eq!(previous_tag(tags, "v1.0.0").as_deref(), Some("v0.10.1"));
        assert_eq!(previous_tag(tags, "v0.10.1").as_deref(), Some("v0.10.0"));
        // A pre-release is not "the previous release": the notes of the next one, and of the
        // pre-release itself, cover the whole cycle since the last release.
        let cycle = ["v0.9.0", "v1.0.0-rc.1", "v1.0.0-rc.2"];
        assert_eq!(
            previous_tag(cycle, "v1.0.0-rc.2").as_deref(),
            Some("v0.9.0")
        );
        assert_eq!(
            previous_tag(["v0.1.0-rc.1", "v0.0.9"], "v0.1.0").as_deref(),
            Some("v0.0.9")
        );
        assert_eq!(
            previous_tag(["v0.1.0-rc.1", "not-a-version"], "v0.1.0"),
            None
        );
    }

    #[test]
    fn pins_come_from_every_manifest_and_are_deduplicated() {
        let core = "# 3dcraft (MIT): reading .3dcraft.\n\
                    craft3d-format = { git = \"https://github.com/johancoppens/3dcraft\", tag = \"v0.1.3\" }\n\
                    craft3d-doc = { git = \"https://github.com/johancoppens/3dcraft\", tag = \"v0.1.3\" }\n\
                    serde = { version = \"1\", features = [\"derive\"] }\n";
        let other = "[dependencies]\n\
                      foo = { git = \"https://github.com/example/foo.git\",\n  rev = \"58bfc60\" }\n\
                      bar = { version = \"1\", git = \"https://github.com/example/bar\", branch = \"main\" }\n\
                      local = { path = \"../local\" }\n\
                      # gone = { git = \"https://github.com/example/gone\", tag = \"v0.0.1\" }\n\
                      plain = \"2\"\n";
        assert_eq!(
            pins([core, other]),
            vec![
                Pin {
                    name: "3dcraft".into(),
                    url: "https://github.com/johancoppens/3dcraft".into(),
                    refs: vec!["tag v0.1.3".into()]
                },
                Pin {
                    name: "bar".into(),
                    url: "https://github.com/example/bar".into(),
                    refs: vec!["branch main".into()]
                },
                Pin {
                    name: "foo".into(),
                    url: "https://github.com/example/foo.git".into(),
                    refs: vec!["rev 58bfc60".into()]
                },
            ]
        );
        assert!(pins(["[dependencies]\nserde = \"1\"\n"]).is_empty());
    }

    #[test]
    fn the_notes_record_the_pins_and_the_asset_library() {
        let pins = [Pin {
            name: "3dcraft".into(),
            url: "https://github.com/johancoppens/3dcraft".into(),
            refs: vec!["tag v0.1.3".into()],
        }];
        let text = notes(&[commit("feat: one", "")], Some("v0.1.0"), &pins);
        assert!(
            text.contains(
                "\n### Pins\n\n- 3dcraft: tag `v0.1.3` (https://github.com/johancoppens/3dcraft)\n"
            ),
            "{text}"
        );
        assert!(text.contains("\n### Asset library\n\n"), "{text}");
        assert!(text.contains("tools/visioncraft-assets"), "{text}");
    }

    #[test]
    fn the_manifests_of_a_tree_skip_build_output_and_hidden_directories() {
        let root = std::env::temp_dir().join(format!("release-notes-test-{}", std::process::id()));
        for dir in [
            "",
            "crates/core",
            "target/debug",
            ".git/objects",
            "node_modules/x",
            "tools/release",
        ] {
            std::fs::create_dir_all(root.join(dir)).expect("create a test directory");
            std::fs::write(root.join(dir).join("Cargo.toml"), "").expect("write a test manifest");
        }
        let mut found: Vec<_> = manifest_paths(&root)
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .expect("under the root")
                    .display()
                    .to_string()
            })
            .collect();
        found.sort();
        std::fs::remove_dir_all(&root).expect("remove the test directory");
        assert_eq!(
            found,
            [
                "Cargo.toml",
                "crates/core/Cargo.toml",
                "tools/release/Cargo.toml"
            ]
        );
    }
}
