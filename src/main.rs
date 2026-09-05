//! `owl-manifest` — the CLI every product org's build runs.
//!
//!   owl-manifest generate --dir dist --app-id gha-indie-worker-web \
//!       --release-id 2026.09.05-abc --framework leptos \
//!       --base-url /assets/releases/2026.09.05-abc/ [--write]
//!
//!   owl-manifest verify --dir dist [--manifest dist/owl-manifest.json]
//!
//! Exit codes are part of the contract: 0 success, 1 the release is wrong, 2 the invocation
//! is wrong.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use owl_manifest_tools::manifest::{build, verify, BuildSpec};
use owl_manifest_tools::scan::{scan_dir, Framework};

const USAGE: &str = "\
owl-manifest generate --dir <build dir> --app-id <id> --release-id <id> \\
              --framework flutter|leptos|dioxus|wasm-bindgen \\
              --base-url </assets/releases/<release-id>/> \\
              [--toolchain <text>] [--activation attach-view|hydrate-islands|mount-route|run-app] \\
              [--host-selector <css>] [--island <name>]... [--route <path>=<chunk>]... \\
              [--furthest-stage fetch|compile] [--max-concurrency <n>] \\
              [--cross-origin-isolated] [--write]

owl-manifest verify   --dir <build dir> [--manifest <path>]";

struct Args {
    command: String,
    values: BTreeMap<String, Vec<String>>,
    flags: Vec<String>,
}

fn parse_args(argv: Vec<String>) -> Result<Args, String> {
    let mut iter = argv.into_iter().skip(1);
    let command = iter.next().ok_or_else(|| USAGE.to_string())?;
    let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut flags = Vec::new();
    let known_flags = ["--write", "--cross-origin-isolated", "--help"];
    let mut pending: Option<String> = None;
    for token in iter {
        match pending.take() {
            Some(key) => values.entry(key).or_default().push(token),
            None => {
                if known_flags.contains(&token.as_str()) {
                    flags.push(token);
                } else if let Some(key) = token.strip_prefix("--") {
                    pending = Some(key.to_string());
                } else {
                    return Err(format!("unexpected argument `{token}`"));
                }
            }
        }
    }
    if let Some(key) = pending {
        return Err(format!("--{key} needs a value"));
    }
    Ok(Args {
        command,
        values,
        flags,
    })
}

impl Args {
    fn one(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .and_then(|v| v.last())
            .map(String::as_str)
    }

    fn required(&self, key: &str) -> Result<&str, String> {
        self.one(key).ok_or_else(|| format!("--{key} is required"))
    }

    fn many(&self, key: &str) -> Vec<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }
}

fn run() -> Result<u8, String> {
    let args = parse_args(std::env::args().collect())?;
    if args.flag("--help") {
        println!("{USAGE}");
        return Ok(0);
    }
    let dir = PathBuf::from(args.required("dir")?);
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let files = scan_dir(&dir)?;
    eprintln!(
        "[owl-manifest] scanned {} files under {}",
        files.len(),
        dir.display()
    );

    match args.command.as_str() {
        "generate" => {
            let framework = Framework::parse(args.required("framework")?)?;
            let release_id = args.required("release-id")?.to_string();
            let mut routes = BTreeMap::new();
            for pair in args.many("route") {
                let (path, chunk) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("--route expects <path>=<chunk>, got `{pair}`"))?;
                routes.insert(path.to_string(), chunk.to_string());
            }
            let spec = BuildSpec {
                app_id: args.required("app-id")?.to_string(),
                base_url: args
                    .one("base-url")
                    .map(String::from)
                    .unwrap_or_else(|| format!("/assets/releases/{release_id}/")),
                release_id,
                framework,
                toolchain: args.one("toolchain").unwrap_or("unspecified").to_string(),
                requires_cross_origin_isolation: args.flag("--cross-origin-isolated"),
                max_concurrency: args
                    .one("max-concurrency")
                    .unwrap_or("4")
                    .parse::<i64>()
                    .map_err(|_| "--max-concurrency must be a number".to_string())?,
                furthest_stage: args.one("furthest-stage").unwrap_or("fetch").to_string(),
                activation_mode: args
                    .one("activation")
                    .unwrap_or(match framework {
                        Framework::Flutter => "attach-view",
                        Framework::Leptos => "hydrate-islands",
                        Framework::Dioxus => "mount-route",
                        Framework::WasmBindgen => "run-app",
                    })
                    .to_string(),
                host_selector: args.one("host-selector").map(String::from),
                islands: args.many("island"),
                routes,
            };
            let manifest = build(&files, &spec)?;
            let text = manifest.to_pretty();
            if args.flag("--write") {
                let out = dir.join("owl-manifest.json");
                std::fs::write(&out, &text).map_err(|e| format!("{}: {e}", out.display()))?;
                eprintln!("[owl-manifest] wrote {}", out.display());
            } else {
                print!("{text}");
            }
            Ok(0)
        }
        "verify" => {
            let path = args
                .one("manifest")
                .map(PathBuf::from)
                .unwrap_or_else(|| dir.join("owl-manifest.json"));
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let report = verify(&text, &files)?;
            eprintln!("[owl-manifest] checked {} listed files", report.checked);
            if report.ok() {
                eprintln!("[owl-manifest] release verified");
                Ok(0)
            } else {
                for problem in &report.problems {
                    eprintln!("[owl-manifest] BLOCKER {problem}");
                }
                eprintln!("[owl-manifest] {} problem(s)", report.problems.len());
                Ok(1)
            }
        }
        other => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("[owl-manifest] {message}");
            ExitCode::from(2)
        }
    }
}
