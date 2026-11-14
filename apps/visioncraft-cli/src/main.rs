//! Headless command-line interface for VisionCraft, shaped after 3dcraft's `craft3d-cli` (MIT):
//! the same subcommands where they mean something here, the same flags, one JSON line per
//! result on stdout, errors as one line on stderr.
//!
//! Every subcommand is a thin wrapper over `visioncraft-automation`: `run`, `batch`, `info`,
//! `render`, `export`, `replay`, `convert` and `new` run commands for a trusted local user;
//! `serve` and `mcp` hand stdin and stdout to the servers, with file access only beneath the
//! roots given.
//!
//! Exit codes: 0 success, 1 failure, 2 usage error.

mod gpu;

use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use visioncraft_automation::{Access, Headless, commands, mcp, rpc};

enum CliError {
    Usage(String),
    Failed(String),
}

type Res = Result<(), CliError>;

const HELP: &str = "\
visioncraft-cli: headless command-line interface for VisionCraft

Usage: visioncraft-cli <subcommand> [options]
       visioncraft-cli <subcommand> --help
       visioncraft-cli --version | --help

Subcommands:
  commands   List the commands: id, params, summary
  run        Open or create a project, run --script and --cmd, save with --out
  batch      Run a script as one batch (the method batch); --atomic: all or nothing
  info       Print a project's state as JSON
  render     Render a project to a PNG (on the GPU)
  export     Render the viewpoints at the export presets (Screen, HD, 4K, ...)
  replay     Render a tour through the viewpoints as numbered PNG frames
  convert    Make a .visioncraft project of a .3dcraft or .obj model, or rewrite one
  new        Write an empty project
  serve      JSON-lines server on stdin/stdout, for scripts
  mcp        Model Context Protocol server on stdin/stdout, for agents

Exit codes: 0 success, 1 failure, 2 usage error.";

const HELP_COMMANDS: &str = "\
Usage: visioncraft-cli commands [--filter TEXT]

Prints the commands, one block each: id, params and summary. These are the ids
`run --cmd`, `serve` and `mcp` accept (`engine.commands` and the MCP tool
command_list give the full parameter docs as JSON).

Options:
  --filter TEXT   Only commands whose id or label contains TEXT (any case)";

const HELP_RUN: &str = "\
Usage: visioncraft-cli run [FILE] [--new JSON] [--script SCRIPT]
                           [--cmd ID [--params JSON]]... [--out FILE] [--keep-going]

Opens the project FILE (a .visioncraft file, as file.open opens it, with the
models it refers to) or starts a new one, runs the steps of the script, then
each --cmd, in order, and prints one line of JSON per result. Stops at the
first command that fails, with exit code 1 and nothing saved, unless
--keep-going is given.

Options:
  --new JSON      Params of file.new for the new project ({} is all it takes).
                  Not with FILE
  --script SCRIPT A JSON file: an array of steps, each [\"id\", {params}] or
                  {\"command\": \"id\", \"params\": {...}, \"agent\": name} (or
                  {\"steps\": [...]}); with an agent, the step is noted as that
                  agent's in the agent log (agent.log)
  --cmd ID        A command to run; may be repeated
  --params JSON   The params of the --cmd before it, as a JSON object
  --out FILE      Save the project afterwards, as file.save does
  --keep-going    Go on after a command that fails: its line is
                  {\"failed\": id, \"step\": n, \"error\": text}; --out still saves
                  and the exit code is 1

Example:
  visioncraft-cli run --cmd shape.add_area \\
      --params '{\"name\": \"Lawn\", \"outline\": [[0,0],[10,0],[10,8],[0,8]]}' \\
      --cmd query.measure --params '{\"kind\": \"area\", \"outline\": [[0,0],[10,0],[10,8],[0,8]]}' \\
      --out garden.visioncraft";

const HELP_BATCH: &str = "\
Usage: visioncraft-cli batch [FILE] --script SCRIPT [--out FILE] [--terse]
                             [--keep-going] [--atomic]

Opens FILE (or starts a new project) and runs the script's steps as the method
`batch` does, printing its report as one line of JSON: in order, stopping at
the first step that fails, with nothing saved (--keep-going: going on, and
--out saves what was built), each step applied on its own. With --atomic it is
all or nothing: the first step that fails takes back every step. Exit code 1
when a step failed.";

const HELP_INFO: &str = "\
Usage: visioncraft-cli info FILE [--no-faces] [--placements] [--full]

Prints the state of a .visioncraft project as JSON (the command
document.inspect): site, time and sun, layers, models with face names, shapes
with areas, materials, planting generators with plant counts, lights,
viewpoints, terrain.

Options:
  --no-faces      Leave out the face names
  --placements    Every plant's position (large)
  --full          Every field of materials, generators and species";

const HELP_RENDER: &str = "\
Usage: visioncraft-cli render FILE --out PNG [--size WxH] [--viewpoint V]
                              [--camera X,Y,Z,TX,TY,TZ] [--fov DEG]
                              [--date YYYY-MM-DD] [--time HH:MM]
                              [--quality low|medium|high|photo] [--samples N]
                              [--aperture M] [--focus M]

Renders the project to a PNG with VisionCraft's renderer on the GPU, without a
window, plants grown, and prints what it drew as one line of JSON
(view.render). The project does not change.

Options:
  --out PNG       The image file to write (required)
  --size WxH      Pixels, each 16 to 4096. Default 1280x720
  --viewpoint V   A viewpoint's id or name (its camera, and its time if it has one)
  --camera ...    Eye and target in metres, instead of a viewpoint
  --fov DEG       Vertical field of view with --camera. Default 50
  --date, --time  Local date and time of the sun, for this picture only
  --quality Q     low, medium, high (default) or photo (many frames averaged
                  at ULTRA: soft shadows, smooth edges; slower)
  --samples N     Photo: frames averaged, 1 to 1024. Default 64
  --aperture M    Photo: lens radius in metres for depth of field (0 to 0.5)
  --focus M       Photo: distance in focus. Default the camera target";

const HELP_EXPORT: &str = "\
Usage: visioncraft-cli export FILE --out DIR [--preset NAME]... [--viewpoint V]... [--years Y]...

Renders every viewpoint (or those given; the whole scene framed when the
project has none) at every preset given (default Screen) into DIR as
<viewpoint>-<preset>.png, one line of JSON per picture. With --years (each
0..60), a growth series: <viewpoint>-<preset>-y<years>.png for every year
given. Presets: {presets}.";

const HELP_REPLAY: &str = "\
Usage: visioncraft-cli replay FILE --out DIR [--size WxH] [--frames-per-step N]
                              [--hold N]

Renders a tour through the project's viewpoints, in their order, eased between
each two (the window's fly-to): DIR/frame-00000.png and on. Prints the result
as one line of JSON. Needs at least one viewpoint. No video: make one with
`ffmpeg -framerate 30 -i DIR/frame-%05d.png tour.mp4`.

Options:
  --size WxH            Default 1280x720
  --frames-per-step N   Frames from one viewpoint to the next. Default 30
  --hold N              Extra frames at each viewpoint. Default 0";

const HELP_CONVERT: &str = "\
Usage: visioncraft-cli convert IN OUT

IN a .3dcraft or .obj model: OUT is a new .visioncraft project with that model
(as model m1, its path relative to OUT). IN a .visioncraft project: OUT is the
same project written again in the current format, model paths following it.";

const HELP_NEW: &str = "\
Usage: visioncraft-cli new FILE [--example NAME] [--force]

Writes an empty project (the built-in materials, the default site and time), or
with --example an example built from commands (garden: a 20 by 30 m plot with
house, terrace, lawn, hedge, border, trees, pond, lamps and six viewpoints).
--force replaces an existing FILE.";

const HELP_SERVE: &str = "\
Usage: visioncraft-cli serve [--automation-read-root DIR] [--automation-write-root DIR]
                             [--control PORT [--control-token-file FILE] [--state-dir DIR]]

Serves the JSON-lines protocol on stdin and stdout until stdin closes: one
request per line, {\"id\": ..., \"method\": ..., \"params\": {...}}, and one reply
per line. Send {\"method\": \"methods\"} for the list of methods.

The session reads files only beneath the read root and writes only beneath
the write root; without one, that direction is denied.

With --control the same session also listens on 127.0.0.1:PORT (0: any free
port, printed on stderr) behind a token, as a window does, so that
`visioncraft-cli mcp --bridge` reaches it; stdin is then not read and the
server runs until it is killed.

Options:
  --automation-read-root DIR    An existing directory commands may read from
  --automation-write-root DIR   An existing directory commands may write to
  --control PORT                Listen for the control channel on PORT
  --control-token-file FILE     The token file (made when missing). Default:
                                control.token in the state folder, with a
                                discovery entry
  --state-dir DIR               The state folder. Default: visioncraft/ in your
                                state directory (VISIONCRAFT_STATE_DIR)";

const HELP_MCP: &str = "\
Usage: visioncraft-cli mcp [--automation-read-root DIR] [--automation-write-root DIR]
       visioncraft-cli mcp --bridge [PORT] [--state-dir DIR]
       visioncraft-cli mcp --bridge PORT --control-token-file FILE

Serves the Model Context Protocol on stdin and stdout (JSON-RPC 2.0, one
message per line), for agent clients. Tools: {tools}, and, bridged to a
window, control_call. Resources: visioncraft://project, visioncraft://guide.
Prompt: build_garden.

With --bridge there is no session here: every message is passed to a running
VisionCraft window (or `serve --control`) and answered on the project it
shows. --bridge alone finds it through its entry in the state folder; a PORT
narrows the choice; --control-token-file skips discovery.";

fn print(text: &str) -> Res {
    let mut out = std::io::stdout().lock();
    out.write_all(text.as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush())
        .map_err(|e| CliError::Failed(format!("cannot write to standard output: {e}")))
}

fn print_json(v: &Value) -> Res {
    print(&serde_json::to_string(v).unwrap_or_default())
}

/// The arguments of one subcommand: positionals, and flags with their values in order.
struct Args {
    positionals: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(raw: &[String], with_value: &[&str], switches: &[&str]) -> Result<Self, CliError> {
        let (mut positionals, mut flags) = (Vec::new(), Vec::new());
        let mut i = 0;
        let mut only_positionals = false;
        while i < raw.len() {
            let a = &raw[i];
            i += 1;
            if only_positionals || !a.starts_with("--") || a == "-" {
                positionals.push(a.clone());
                continue;
            }
            if a == "--" {
                only_positionals = true;
                continue;
            }
            let (name, inline) = match a.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (a.clone(), None),
            };
            if name == "--help" {
                flags.push((name, None));
            } else if switches.contains(&name.as_str()) {
                if inline.is_some() {
                    return Err(CliError::Usage(format!("`{name}` takes no value")));
                }
                flags.push((name, None));
            } else if name == "--bridge" && with_value.contains(&"--bridge") {
                // An optional PORT.
                let value = inline.or_else(|| {
                    raw.get(i)
                        .filter(|n| !n.starts_with("--"))
                        .cloned()
                        .inspect(|_| i += 1)
                });
                flags.push((name, value));
            } else if with_value.contains(&name.as_str()) {
                let value = match inline {
                    Some(v) => v,
                    None => {
                        let v = raw
                            .get(i)
                            .cloned()
                            .ok_or_else(|| CliError::Usage(format!("`{name}` needs a value")))?;
                        i += 1;
                        v
                    }
                };
                flags.push((name, Some(value)));
            } else {
                return Err(CliError::Usage(format!("unknown flag `{name}`")));
            }
        }
        Ok(Self { positionals, flags })
    }

    fn help(&self) -> bool {
        self.flags.iter().any(|(n, _)| n == "--help")
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }

    fn value(&self, name: &str) -> Result<Option<&str>, CliError> {
        let mut found = self.flags.iter().filter(|(n, _)| n == name);
        let first = found.next().and_then(|(_, v)| v.as_deref());
        if found.next().is_some() {
            return Err(CliError::Usage(format!("`{name}` given twice")));
        }
        Ok(first)
    }

    fn all(&self, name: &str) -> Vec<&str> {
        self.flags
            .iter()
            .filter(|(n, _)| n == name)
            .filter_map(|(_, v)| v.as_deref())
            .collect()
    }

    fn at_most(&self, n: usize) -> Result<(), CliError> {
        match self.positionals.get(n) {
            Some(extra) => Err(CliError::Usage(format!("unexpected argument `{extra}`"))),
            None => Ok(()),
        }
    }
}

/// A host for a trusted local user: any file, pictures on the GPU.
fn local_host() -> Headless {
    let mut host = Headless::new(Access::any());
    host.pictures = Some(Box::new(gpu::GpuPictures::default()));
    host
}

fn exec(host: &mut Headless, id: &str, params: Value) -> Result<Value, CliError> {
    commands::execute(host, id, &params).map_err(CliError::Failed)
}

fn open_or_new(host: &mut Headless, file: Option<&String>) -> Res {
    if let Some(f) = file {
        let r = exec(host, "file.open", json!({ "path": f }))?;
        for m in r["missingModels"].as_array().into_iter().flatten() {
            eprintln!(
                "visioncraft-cli: warning: model {} not loaded: {}",
                m["id"],
                m["error"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

/// One step of a script: the command, its params, and the agent it is noted for (`agent`).
struct ScriptStep {
    id: String,
    params: Value,
    agent: Option<String>,
}

/// Steps from a script file: `["id", {params}]` or `{"command", "params", "agent"}`, in an
/// array or under `steps` (craft3d-cli's script format).
fn read_script(path: &str) -> Result<Vec<ScriptStep>, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::Failed(format!("cannot read the script `{path}`: {e}")))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| CliError::Failed(format!("the script `{path}` is not JSON: {e}")))?;
    let list = match &v {
        Value::Array(a) => a,
        Value::Object(o) => o
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| CliError::Failed(format!("the script `{path}` has no `steps` array")))?,
        _ => {
            return Err(CliError::Failed(format!(
                "the script `{path}` must be an array of steps"
            )));
        }
    };
    list.iter()
        .enumerate()
        .map(|(i, s)| {
            let bad = || CliError::Failed(format!("{path}: step {i} must be [\"id\", {{params}}] or {{\"command\": \"id\", \"params\": {{...}}}}"));
            match s {
                Value::Array(a) => {
                    let id = a.first().and_then(Value::as_str).ok_or_else(bad)?;
                    Ok(ScriptStep { id: id.to_string(), params: a.get(1).cloned().unwrap_or(Value::Null), agent: None })
                }
                Value::Object(o) => {
                    let id = o.get("command").and_then(Value::as_str).ok_or_else(bad)?;
                    if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "command" | "params" | "agent")) {
                        return Err(CliError::Failed(format!("{path}: step {i} has unknown key `{k}`")));
                    }
                    let agent = match o.get("agent") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(a)) => Some(a.clone()),
                        Some(_) => return Err(CliError::Failed(format!("{path}: step {i}: `agent` must be a string"))),
                    };
                    Ok(ScriptStep { id: id.to_string(), params: o.get("params").cloned().unwrap_or(Value::Null), agent })
                }
                _ => Err(bad()),
            }
        })
        .collect()
}

fn cmd_commands(raw: &[String]) -> Res {
    let a = Args::parse(raw, &["--filter"], &[])?;
    if a.help() {
        return print(HELP_COMMANDS);
    }
    a.at_most(0)?;
    let filter = a.value("--filter")?.map(str::to_lowercase);
    let host = Headless::new(Access::default());
    let blocks: Vec<String> = commands::docs(&host.session)
        .iter()
        .filter(|d| {
            filter.as_deref().is_none_or(|f| {
                d.id.to_lowercase().contains(f) || d.label.to_lowercase().contains(f)
            })
        })
        .map(|d| {
            format!(
                "{}\n  params:  {}\n  summary: {}",
                d.id,
                d.notation(),
                d.summary
            )
        })
        .collect();
    if blocks.is_empty() {
        return print(&format!(
            "No command matches `{}`.",
            filter.unwrap_or_default()
        ));
    }
    print(&blocks.join("\n\n"))
}

fn cmd_run(raw: &[String]) -> Res {
    let a = Args::parse(
        raw,
        &["--new", "--script", "--cmd", "--params", "--out"],
        &["--keep-going"],
    )?;
    if a.help() {
        return print(HELP_RUN);
    }
    a.at_most(1)?;
    let mut steps = match a.value("--script")? {
        Some(s) => read_script(s)?,
        None => Vec::new(),
    };
    let mut after_cmd = false;
    for (name, value) in &a.flags {
        match (name.as_str(), value) {
            ("--cmd", Some(id)) => {
                steps.push(ScriptStep {
                    id: id.clone(),
                    params: Value::Null,
                    agent: None,
                });
                after_cmd = true;
            }
            ("--params", Some(p)) => {
                let last = steps
                    .last_mut()
                    .filter(|_| after_cmd)
                    .ok_or_else(|| CliError::Usage("`--params` must follow a `--cmd`".into()))?;
                if !last.params.is_null() {
                    return Err(CliError::Usage("one `--params` per `--cmd`".into()));
                }
                last.params = serde_json::from_str(p)
                    .map_err(|e| CliError::Usage(format!("`--params` is not JSON: {e}")))?;
            }
            _ => {}
        }
    }
    let keep_going = a.has("--keep-going");
    let mut host = local_host();
    match (a.value("--new")?, a.positionals.first()) {
        (Some(_), Some(_)) => {
            return Err(CliError::Usage(
                "`--new` starts a new project; it does not go with FILE".into(),
            ));
        }
        (Some(new), None) => {
            let params: Value = serde_json::from_str(new)
                .map_err(|e| CliError::Usage(format!("`--new` is not JSON: {e}")))?;
            exec(&mut host, "file.new", params)?;
        }
        (None, file) => open_or_new(&mut host, file)?,
    }
    let mut failed: Vec<String> = Vec::new();
    for (index, ScriptStep { id, params, agent }) in steps.iter().enumerate() {
        // A step with an agent is noted as that agent's in the agent log.
        host.state.client = agent.clone();
        let outcome =
            commands::execute_logged(&mut host, id, params, None).map_err(CliError::Failed);
        match outcome {
            Ok(result) => print_json(&result)?,
            Err(CliError::Failed(error)) if keep_going => {
                print_json(&json!({"failed": id, "step": index, "error": error}))?;
                failed.push(format!("step {index} `{id}`: {error}"));
            }
            Err(e) => return Err(e),
        }
    }
    if let Some(out) = a.value("--out")? {
        exec(
            &mut host,
            "file.save",
            json!({"path": out, "overwrite": true}),
        )?;
    }
    match failed.first() {
        Some(first) => Err(CliError::Failed(format!(
            "{} of {} commands failed, the first {first}",
            failed.len(),
            steps.len()
        ))),
        None => Ok(()),
    }
}

fn cmd_batch(raw: &[String]) -> Res {
    let a = Args::parse(
        raw,
        &["--script", "--out"],
        &["--terse", "--atomic", "--keep-going"],
    )?;
    if a.help() {
        return print(HELP_BATCH);
    }
    a.at_most(1)?;
    let script = a
        .value("--script")?
        .ok_or_else(|| CliError::Usage("missing `--script`".into()))?;
    let steps: Vec<commands::Step> = read_script(script)?
        .into_iter()
        .map(|s| commands::Step {
            command: s.id,
            params: if s.params.is_null() {
                json!({})
            } else {
                s.params
            },
            intent: None,
        })
        .collect();
    let mut host = local_host();
    open_or_new(&mut host, a.positionals.first())?;
    let options = commands::BatchOptions {
        stop_on_error: !a.has("--keep-going"),
        terse: a.has("--terse"),
        atomic: a.has("--atomic"),
        ..Default::default()
    };
    let outcome = commands::run_batch(&mut host, &steps, &options);
    print_json(&outcome.report)?;
    if outcome.failed && options.stop_on_error {
        let why = if options.atomic {
            "the batch failed and was rolled back"
        } else {
            "a step of the batch failed"
        };
        return Err(CliError::Failed(format!("{why}; nothing was saved")));
    }
    if let Some(out) = a.value("--out")? {
        exec(
            &mut host,
            "file.save",
            json!({"path": out, "overwrite": true}),
        )?;
    }
    if outcome.failed {
        return Err(CliError::Failed("a step of the batch failed".into()));
    }
    Ok(())
}

fn cmd_info(raw: &[String]) -> Res {
    let a = Args::parse(raw, &[], &["--no-faces", "--placements", "--full"])?;
    if a.help() {
        return print(HELP_INFO);
    }
    a.at_most(1)?;
    let file = a
        .positionals
        .first()
        .ok_or_else(|| CliError::Usage("missing FILE".into()))?;
    let mut host = Headless::new(Access::any());
    open_or_new(&mut host, Some(file))?;
    let r = exec(
        &mut host,
        "document.inspect",
        json!({"faces": !a.has("--no-faces"), "placements": a.has("--placements"), "full": a.has("--full")}),
    )?;
    print(&serde_json::to_string_pretty(&r).unwrap_or_default())
}

fn parse_size(text: &str) -> Result<(u32, u32), CliError> {
    text.split_once(['x', 'X'])
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .ok_or_else(|| {
            CliError::Usage(format!(
                "`--size` must be WIDTHxHEIGHT in pixels, such as 1280x720, got `{text}`"
            ))
        })
}

fn time_params(date: Option<&str>, time: Option<&str>) -> Result<Option<Value>, CliError> {
    let mut t = serde_json::Map::new();
    if let Some(d) = date {
        let parts: Vec<&str> = d.split('-').collect();
        let [y, m, day] = parts.as_slice() else {
            return Err(CliError::Usage(format!(
                "`--date` must be YYYY-MM-DD, got `{d}`"
            )));
        };
        let n = |s: &str| {
            s.parse::<i64>()
                .map_err(|_| CliError::Usage(format!("`--date` must be YYYY-MM-DD, got `{d}`")))
        };
        t.insert("year".into(), n(y)?.into());
        t.insert("month".into(), n(m)?.into());
        t.insert("day".into(), n(day)?.into());
    }
    if let Some(hm) = time {
        let (h, m) = hm.split_once(':').unwrap_or((hm, "0"));
        let (h, m): (f64, f64) = (
            h.parse()
                .map_err(|_| CliError::Usage(format!("`--time` must be HH:MM, got `{hm}`")))?,
            m.parse()
                .map_err(|_| CliError::Usage(format!("`--time` must be HH:MM, got `{hm}`")))?,
        );
        t.insert("hour".into(), (h + m / 60.0).into());
    }
    Ok((!t.is_empty()).then_some(Value::Object(t)))
}

fn cmd_render(raw: &[String]) -> Res {
    let a = Args::parse(
        raw,
        &[
            "--out",
            "--size",
            "--viewpoint",
            "--camera",
            "--fov",
            "--date",
            "--time",
            "--quality",
            "--samples",
            "--aperture",
            "--focus",
        ],
        &[],
    )?;
    if a.help() {
        return print(HELP_RENDER);
    }
    a.at_most(1)?;
    let file = a
        .positionals
        .first()
        .ok_or_else(|| CliError::Usage("missing FILE".into()))?;
    let out = a
        .value("--out")?
        .ok_or_else(|| CliError::Usage("missing `--out PNG`".into()))?;
    let (w, h) = a.value("--size")?.map_or(Ok((1280, 720)), parse_size)?;
    let mut params = json!({"path": out, "width": w, "height": h});
    if let Some(v) = a.value("--viewpoint")? {
        params["viewpoint"] = v.into();
    }
    if let Some(c) = a.value("--camera")? {
        let n: Vec<f32> = c
            .split(',')
            .map(|x| x.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| {
                CliError::Usage(format!(
                    "`--camera` must be six numbers X,Y,Z,TX,TY,TZ, got `{c}`"
                ))
            })?;
        let [x, y, z, tx, ty, tz] = n.as_slice() else {
            return Err(CliError::Usage(format!(
                "`--camera` must be six numbers X,Y,Z,TX,TY,TZ, got `{c}`"
            )));
        };
        let fov: f32 = a
            .value("--fov")?
            .map_or(Ok(50.0), str::parse)
            .map_err(|_| CliError::Usage("`--fov` must be a number of degrees".into()))?;
        params["camera"] = json!({"position": [x, y, z], "target": [tx, ty, tz], "fov_y_deg": fov});
    }
    if let Some(t) = time_params(a.value("--date")?, a.value("--time")?)? {
        params["time"] = t;
    }
    if let Some(q) = a.value("--quality")? {
        params["quality"] = q.into();
    }
    for (flag, key) in [
        ("--samples", "samples"),
        ("--aperture", "aperture_m"),
        ("--focus", "focus_m"),
    ] {
        if let Some(v) = a.value(flag)? {
            let n: f64 = v
                .parse()
                .map_err(|_| CliError::Usage(format!("`{flag}` must be a number, got `{v}`")))?;
            params[key] = if key == "samples" {
                json!(n as u64)
            } else {
                json!(n)
            };
        }
    }
    let mut host = local_host();
    open_or_new(&mut host, Some(file))?;
    let r = exec(&mut host, "view.render", params)?;
    print_json(&r)
}

fn slug(text: &str) -> String {
    let s: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    s.split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn cmd_export(raw: &[String]) -> Res {
    let a = Args::parse(raw, &["--out", "--preset", "--viewpoint", "--years"], &[])?;
    let names: Vec<&str> = visioncraft_core::export_presets()
        .iter()
        .map(|p| p.0)
        .collect();
    if a.help() {
        return print(&HELP_EXPORT.replace("{presets}", &names.join(", ")));
    }
    a.at_most(1)?;
    let file = a
        .positionals
        .first()
        .ok_or_else(|| CliError::Usage("missing FILE".into()))?;
    let dir = a
        .value("--out")?
        .ok_or_else(|| CliError::Usage("missing `--out DIR`".into()))?;
    let mut presets = Vec::new();
    for p in a.all("--preset") {
        let found = visioncraft_core::export_presets()
            .iter()
            .find(|x| x.0.eq_ignore_ascii_case(p));
        presets.push(*found.ok_or_else(|| {
            CliError::Usage(format!(
                "unknown preset `{p}`; the presets are: {}",
                names.join(", ")
            ))
        })?);
    }
    if presets.is_empty() {
        presets.push(visioncraft_core::export_presets()[0]);
    }
    let mut host = local_host();
    open_or_new(&mut host, Some(file))?;
    let wanted = a.all("--viewpoint");
    let mut views: Vec<(Option<String>, String)> = host
        .session
        .project()
        .viewpoints
        .iter()
        .filter(|v| {
            wanted.is_empty()
                || wanted.contains(&v.id.as_str())
                || wanted.contains(&v.name.as_str())
        })
        .map(|v| (Some(v.id.clone()), slug(&v.name)))
        .collect();
    if views.is_empty() {
        if !wanted.is_empty() {
            return Err(CliError::Failed(format!(
                "no viewpoint {}",
                wanted.join(", ")
            )));
        }
        views.push((None, "scene".into()));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| CliError::Failed(format!("cannot make `{dir}`: {e}")))?;
    let mut years: Vec<Option<f32>> = Vec::new();
    for y in a.all("--years") {
        let v: f32 = y
            .parse()
            .ok()
            .filter(|v| (0.0..=visioncraft_core::YEARS_MAX).contains(v))
            .ok_or_else(|| CliError::Usage(format!("`--years {y}`: a number from 0 to 60")))?;
        years.push(Some(v));
    }
    if years.is_empty() {
        years.push(None);
    }
    for (vp, name) in &views {
        for (preset, w, h) in &presets {
            for y in &years {
                let file = match y {
                    Some(y) => format!("{name}-{}-y{y}.png", slug(preset)),
                    None => format!("{name}-{}.png", slug(preset)),
                };
                let path = Path::new(dir).join(file);
                let mut params = json!({"path": path.to_string_lossy(), "width": w, "height": h});
                if let Some(vp) = vp {
                    params["viewpoint"] = vp.as_str().into();
                }
                if let Some(y) = y {
                    params["years"] = (*y).into();
                }
                let mut r = exec(&mut host, "view.render", params)?;
                r["preset"] = (*preset).into();
                print_json(&r)?;
            }
        }
    }
    Ok(())
}

fn cmd_replay(raw: &[String]) -> Res {
    let a = Args::parse(
        raw,
        &["--out", "--size", "--frames-per-step", "--hold"],
        &[],
    )?;
    if a.help() {
        return print(HELP_REPLAY);
    }
    a.at_most(1)?;
    let file = a
        .positionals
        .first()
        .ok_or_else(|| CliError::Usage("missing FILE".into()))?;
    let dir = a
        .value("--out")?
        .ok_or_else(|| CliError::Usage("missing `--out DIR`".into()))?;
    let (w, h) = a.value("--size")?.map_or(Ok((1280, 720)), parse_size)?;
    let number = |flag: &str, default: usize| -> Result<usize, CliError> {
        a.value(flag)?.map_or(Ok(default), |v| {
            v.parse()
                .map_err(|_| CliError::Usage(format!("`{flag}` must be a whole number")))
        })
    };
    let (per_step, hold) = (
        number("--frames-per-step", 30)?.max(1),
        number("--hold", 0)?,
    );
    let mut host = local_host();
    open_or_new(&mut host, Some(file))?;
    let vps = host.session.project().viewpoints.clone();
    if vps.is_empty() {
        return Err(CliError::Failed(
            "the project has no viewpoints to tour (viewpoint.add)".into(),
        ));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| CliError::Failed(format!("cannot make `{dir}`: {e}")))?;
    let mut cameras = Vec::new();
    for (i, vp) in vps.iter().enumerate() {
        cameras.extend(std::iter::repeat_n(
            (vp.position, vp.target, vp.fov_y_deg),
            hold + 1,
        ));
        if let Some(next) = vps.get(i + 1) {
            for f in 1..per_step {
                let (p, t) = visioncraft_core::interpolate(vp, next, f as f32 / per_step as f32);
                let fov =
                    vp.fov_y_deg + (next.fov_y_deg - vp.fov_y_deg) * f as f32 / per_step as f32;
                cameras.push((p, t, fov));
            }
        }
    }
    for (i, (p, t, fov)) in cameras.iter().enumerate() {
        let path = Path::new(dir).join(format!("frame-{i:05}.png"));
        exec(
            &mut host,
            "view.render",
            json!({"path": path.to_string_lossy(), "width": w, "height": h, "camera": {"position": p, "target": t, "fov_y_deg": fov}}),
        )?;
    }
    print_json(
        &json!({"dir": dir, "frames": cameras.len(), "framePattern": "frame-%05d.png", "width": w, "height": h, "viewpoints": vps.iter().map(|v| v.id.clone()).collect::<Vec<_>>()}),
    )
}

fn cmd_convert(raw: &[String]) -> Res {
    let a = Args::parse(raw, &[], &[])?;
    if a.help() {
        return print(HELP_CONVERT);
    }
    a.at_most(2)?;
    let [input, output] = a.positionals.as_slice() else {
        return Err(CliError::Usage("give IN and OUT".into()));
    };
    let ext = |p: &str| {
        Path::new(p)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default()
    };
    if ext(output) != "visioncraft" {
        return Err(CliError::Usage(format!(
            "OUT must be a .visioncraft file, got `{output}`"
        )));
    }
    let mut host = local_host();
    match ext(input).as_str() {
        "visioncraft" => open_or_new(&mut host, Some(input))?,
        kind @ ("3dcraft" | "obj") => {
            let abs = std::path::absolute(input).map_err(|e| CliError::Failed(e.to_string()))?;
            let name = Path::new(input)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Model")
                .to_string();
            exec(
                &mut host,
                "model.add",
                json!({"name": name, "kind": kind, "source": abs.to_string_lossy()}),
            )?;
        }
        other => {
            return Err(CliError::Usage(format!(
                "IN must be .visioncraft, .3dcraft or .obj, got `.{other}`"
            )));
        }
    }
    let r = exec(
        &mut host,
        "file.save",
        json!({"path": output, "overwrite": true}),
    )?;
    print_json(&r)
}

fn cmd_new(raw: &[String]) -> Res {
    let a = Args::parse(raw, &["--example"], &["--force"])?;
    if a.help() {
        return print(HELP_NEW);
    }
    a.at_most(1)?;
    let file = a
        .positionals
        .first()
        .ok_or_else(|| CliError::Usage("missing FILE".into()))?;
    let mut host = Headless::new(Access::any());
    if let Some(name) = a.value("--example")? {
        exec(&mut host, "project.load_example", json!({"name": name}))?;
    }
    let r = exec(
        &mut host,
        "file.save",
        json!({"path": file, "overwrite": a.has("--force")}),
    )?;
    print_json(&r)
}

fn roots(a: &Args) -> Result<Access, CliError> {
    let dir = |flag: &str| -> Result<Option<PathBuf>, CliError> {
        match a.value(flag)? {
            None => Ok(None),
            Some(d) if Path::new(d).is_dir() => Ok(Some(PathBuf::from(d))),
            Some(d) => Err(CliError::Usage(format!(
                "`{flag}`: `{d}` is not an existing directory"
            ))),
        }
    };
    Ok(Access::roots(
        dir("--automation-read-root")?,
        dir("--automation-write-root")?,
    ))
}

fn cmd_serve(raw: &[String], mcp_protocol: bool) -> Res {
    let mut flags = vec![
        "--automation-read-root",
        "--automation-write-root",
        "--control-token-file",
        "--state-dir",
    ];
    flags.push(if mcp_protocol {
        "--bridge"
    } else {
        "--control"
    });
    let a = Args::parse(raw, &flags, &[])?;
    if a.help() {
        return print(&if mcp_protocol {
            HELP_MCP.replace("{tools}", &mcp::tool_names().join(", "))
        } else {
            HELP_SERVE.to_string()
        });
    }
    a.at_most(0)?;
    let port = |text: &str| -> Result<u16, CliError> {
        let t = text.strip_prefix("127.0.0.1:").unwrap_or(text);
        if t.contains(':') {
            return Err(CliError::Usage(format!(
                "`{text}`: only 127.0.0.1 is allowed: the token travels as it is"
            )));
        }
        t.parse()
            .map_err(|_| CliError::Usage(format!("`{text}` is not a port")))
    };
    let state_dir = a.value("--state-dir")?.map(PathBuf::from);
    let token_file = a.value("--control-token-file")?.map(PathBuf::from);
    let (stdin, stdout) = (std::io::stdin().lock(), std::io::stdout().lock());
    let io = |e: std::io::Error| CliError::Failed(format!("standard input or output failed: {e}"));
    if mcp_protocol && a.has("--bridge") {
        use visioncraft_automation::bridge::{BridgeConfig, serve};
        let given = a.value("--bridge")?.map(port).transpose()?;
        let config = match (given, token_file) {
            (Some(p), Some(file)) => {
                let token = visioncraft_automation::discovery::read_token(&file)
                    .map_err(CliError::Failed)?;
                BridgeConfig::new(std::net::SocketAddr::from(([127, 0, 0, 1], p)), token)
            }
            (None, Some(_)) => {
                return Err(CliError::Usage(
                    "`--control-token-file` needs the PORT after `--bridge`".into(),
                ));
            }
            (p, None) => {
                let folder = state_dir
                    .or_else(visioncraft_automation::discovery::state_folder)
                    .ok_or_else(|| CliError::Failed("no state folder; give --state-dir".into()))?;
                BridgeConfig::discover(folder, p)
            }
        };
        return serve(&config, stdin, stdout).map_err(io);
    }
    let mut host = Headless::new(roots(&a)?);
    host.pictures = Some(Box::new(gpu::GpuPictures::default()));
    if let Some(p) = a.value("--control")? {
        use visioncraft_automation::control::{ControlOptions, serve_control};
        let handle = serve_control(ControlOptions {
            port: Some(port(p)?),
            token_file,
            state_dir,
        })
        .map_err(CliError::Failed)?;
        eprintln!(
            "visioncraft-cli: control channel on 127.0.0.1:{} (token file {})",
            handle.port(),
            handle.token_file().display()
        );
        loop {
            handle.serve_one(&mut host, std::time::Duration::from_secs(3600));
        }
    }
    if mcp_protocol {
        mcp::serve(&mut host, stdin, stdout).map_err(io)
    } else {
        rpc::serve(&mut host, stdin, stdout).map_err(io)
    }
}

fn run(raw: &[String]) -> Res {
    let Some(first) = raw.first() else {
        return print(HELP);
    };
    let rest = &raw[1..];
    let name = first.as_str();
    let ran = match name {
        "--help" | "-h" | "help" => return print(HELP),
        "--version" | "-V" => {
            return print(&format!("visioncraft-cli {}", env!("CARGO_PKG_VERSION")));
        }
        "commands" => cmd_commands(rest),
        "run" => cmd_run(rest),
        "batch" => cmd_batch(rest),
        "info" => cmd_info(rest),
        "render" => cmd_render(rest),
        "export" => cmd_export(rest),
        "replay" => cmd_replay(rest),
        "convert" => cmd_convert(rest),
        "new" => cmd_new(rest),
        "serve" => cmd_serve(rest, false),
        "mcp" => cmd_serve(rest, true),
        other if other.starts_with('-') => {
            return Err(CliError::Usage(format!(
                "unknown flag `{other}`; a subcommand comes first"
            )));
        }
        other => {
            return Err(CliError::Usage(format!(
                "unknown subcommand `{other}`; the subcommands are: commands, run, batch, info, render, export, replay, convert, new, serve, mcp"
            )));
        }
    };
    ran.map_err(|e| match e {
        CliError::Usage(m) => {
            CliError::Usage(format!("{name}: {m}\nTry `visioncraft-cli {name} --help`."))
        }
        other => other,
    })
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    match run(&raw) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(m)) => {
            let _ = writeln!(std::io::stderr(), "visioncraft-cli: {m}");
            ExitCode::from(2)
        }
        Err(CliError::Failed(m)) => {
            let _ = writeln!(std::io::stderr(), "visioncraft-cli: {m}");
            ExitCode::FAILURE
        }
    }
}
