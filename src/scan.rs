//! Reading a real build directory.
//!
//! The rule this module exists to keep: the manifest describes what the toolchain actually
//! emitted. Roles are assigned from the files that are there, and a build whose shape the
//! tool does not recognize is an error naming what it found — never a manifest full of
//! guessed filenames that fails later, in a browser, at a visitor's expense.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::sha256::{hex, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub content_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framework {
    Flutter,
    Leptos,
    Dioxus,
    WasmBindgen,
}

impl Framework {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "flutter" => Ok(Self::Flutter),
            "leptos" => Ok(Self::Leptos),
            "dioxus" => Ok(Self::Dioxus),
            "wasm-bindgen" => Ok(Self::WasmBindgen),
            other => Err(format!(
                "unknown framework `{other}` (expected flutter, leptos, dioxus or wasm-bindgen)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flutter => "flutter",
            Self::Leptos => "leptos",
            Self::Dioxus => "dioxus",
            Self::WasmBindgen => "wasm-bindgen",
        }
    }
}

/// Content types that matter to the loader. `application/wasm` above all: streaming
/// instantiation depends on it, and a wrong type degrades silently.
pub fn content_type_for(path: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "wasm" => "application/wasm",
        "js" | "mjs" => "text/javascript",
        "json" | "map" => "application/json",
        "css" => "text/css",
        "html" => "text/html",
        "otf" => "font/otf",
        "ttf" => "font/ttf",
        "woff2" => "font/woff2",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "bin" | "dat" => "application/octet-stream",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn hash_file(path: &Path) -> std::io::Result<(u64, String)> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((total, hex(&hasher.finish())))
}

/// Every regular file under `root`, sorted, with size and digest. Hidden files and the
/// manifest itself are skipped so regenerating a manifest is idempotent.
pub fn scan_dir(root: &Path) -> Result<Vec<ScannedFile>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "owl-manifest.json" {
                continue;
            }
            let meta =
                fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let rel = relative(root, &path)?;
                let (bytes, digest) =
                    hash_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                out.push(ScannedFile {
                    content_type: content_type_for(&rel),
                    path: rel,
                    bytes,
                    sha256: digest,
                });
            }
            // Symlinks are deliberately ignored: a release must be a self-contained tree.
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn relative(root: &Path, path: &Path) -> Result<String, String> {
    let rel: PathBuf = path
        .strip_prefix(root)
        .map_err(|_| format!("{} is outside {}", path.display(), root.display()))?
        .to_path_buf();
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/"))
}

/// A file with the role its build output gives it at startup.
pub type Entrypoint = (ScannedFile, Role);

/// A classified build tree: the entrypoints, and everything else.
pub type Classified = (Vec<Entrypoint>, Vec<ScannedFile>);

/// The role a file plays at startup, decided by framework and by what is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Bootstrap,
    Glue,
    Module,
    Fallback,
    Chunk,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::Glue => "glue",
            Self::Module => "module",
            Self::Fallback => "fallback",
            Self::Chunk => "chunk",
        }
    }
}

/// Classify the scanned tree into (entrypoint, role) pairs plus the remaining assets.
///
/// Flutter's output is named by its own toolchain, so those names are matched exactly.
/// wasm-bindgen output is matched structurally instead — `<name>.js` beside `<name>_bg.wasm`
/// — because the crate name differs per app and guessing it would be wrong more often than
/// right.
pub fn classify(files: &[ScannedFile], framework: Framework) -> Result<Classified, String> {
    let mut entries: Vec<Entrypoint> = Vec::new();

    match framework {
        Framework::Flutter => {
            for file in files {
                let role = match file.path.as_str() {
                    "flutter_bootstrap.js" => Some(Role::Bootstrap),
                    "main.dart.wasm" => Some(Role::Module),
                    "main.dart.mjs" => Some(Role::Glue),
                    "main.dart.js" => Some(Role::Fallback),
                    _ => None,
                };
                if let Some(role) = role {
                    entries.push((file.clone(), role));
                }
            }
            if !entries.iter().any(|(_, r)| *r == Role::Bootstrap) {
                return Err(format!(
                    "no flutter_bootstrap.js in the build output (found: {})",
                    preview(files)
                ));
            }
            if !entries.iter().any(|(_, r)| *r == Role::Module) {
                return Err(format!(
                    "no main.dart.wasm: this looks like a JS-only Flutter build, which needs no Wasm loader (found: {})",
                    preview(files)
                ));
            }
        }
        Framework::Leptos | Framework::Dioxus | Framework::WasmBindgen => {
            let modules: Vec<&ScannedFile> = files
                .iter()
                .filter(|f| f.path.ends_with("_bg.wasm") && !f.path.contains('/'))
                .collect();
            let module = match modules.as_slice() {
                [one] => *one,
                [] => {
                    return Err(format!(
                        "no `<name>_bg.wasm` at the top of the build output — is this a wasm-bindgen build? (found: {})",
                        preview(files)
                    ))
                }
                many => {
                    return Err(format!(
                        "{} candidate modules ({}) — a release must have exactly one entry module",
                        many.len(),
                        many.iter().map(|f| f.path.as_str()).collect::<Vec<_>>().join(", ")
                    ))
                }
            };
            let stem = module.path.trim_end_matches("_bg.wasm").to_string();
            let glue = files
                .iter()
                .find(|f| f.path == format!("{stem}.js") || f.path == format!("{stem}.mjs"))
                .ok_or_else(|| {
                    format!("found {} but no `{stem}.js` glue beside it: wasm-bindgen emits both, and the pair is the release", module.path)
                })?;
            entries.push((glue.clone(), Role::Glue));
            entries.push((module.clone(), Role::Module));

            // Route/lazy chunks, when the framework's splitter emitted any.
            for file in files {
                if file.path.starts_with("chunks/") && file.path.ends_with(".wasm") {
                    entries.push((file.clone(), Role::Chunk));
                }
            }
        }
    }

    let claimed: Vec<String> = entries.iter().map(|(f, _)| f.path.clone()).collect();
    let assets = files
        .iter()
        .filter(|f| !claimed.contains(&f.path))
        .cloned()
        .collect();
    Ok((entries, assets))
}

fn preview(files: &[ScannedFile]) -> String {
    let names: Vec<&str> = files.iter().take(8).map(|f| f.path.as_str()).collect();
    if files.len() > names.len() {
        format!("{}, … {} more", names.join(", "), files.len() - names.len())
    } else {
        names.join(", ")
    }
}

/// How eagerly an asset may be prepared.
///
/// Conservative on purpose: only the small structural files a startup always reads are
/// `critical`, renderer and font payloads are `optional`, and anything that looks like
/// on-demand content is `lazy` and never prepared speculatively.
pub fn stage_for(path: &str, bytes: u64) -> &'static str {
    let critical = path.ends_with("AssetManifest.bin.json")
        || path.ends_with("AssetManifest.json")
        || path.ends_with("FontManifest.json")
        || path.ends_with(".css");
    if critical {
        return "critical";
    }
    let optional = path.starts_with("canvaskit/")
        || path.starts_with("snippets/")
        || path.contains("/fonts/")
        || path.ends_with(".woff2")
        || path.ends_with(".otf")
        || path.ends_with(".ttf");
    if optional && bytes <= 4 * 1024 * 1024 {
        return "optional";
    }
    "lazy"
}

#[cfg(test)]
mod tests {
    use super::{classify, content_type_for, stage_for, Framework, ScannedFile};

    fn file(path: &str, bytes: u64) -> ScannedFile {
        ScannedFile {
            path: path.into(),
            bytes,
            sha256: "0".repeat(64),
            content_type: content_type_for(path),
        }
    }

    #[test]
    fn flutter_output_is_recognized_by_its_toolchain_names() {
        let files = vec![
            file("flutter_bootstrap.js", 10),
            file("main.dart.wasm", 100),
            file("main.dart.mjs", 20),
            file("main.dart.js", 90),
            file("assets/FontManifest.json", 5),
        ];
        let (entries, assets) = classify(&files, Framework::Flutter).expect("classifies");
        assert_eq!(entries.len(), 4);
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].path, "assets/FontManifest.json");
    }

    #[test]
    fn a_js_only_flutter_build_is_refused_with_a_reason() {
        let files = vec![file("flutter_bootstrap.js", 10), file("main.dart.js", 90)];
        let err = classify(&files, Framework::Flutter).expect_err("should refuse");
        assert!(err.contains("no main.dart.wasm"), "{err}");
        assert!(
            err.contains("main.dart.js"),
            "the error should show what was found: {err}"
        );
    }

    #[test]
    fn wasm_bindgen_output_is_recognized_structurally_whatever_the_crate_is_called() {
        let files = vec![
            file("some_app.js", 10),
            file("some_app_bg.wasm", 100),
            file("chunks/reports.wasm", 50),
            file("app.css", 5),
        ];
        let (entries, assets) = classify(&files, Framework::Dioxus).expect("classifies");
        let roles: Vec<&str> = entries.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(roles, vec!["glue", "module", "chunk"]);
        assert_eq!(assets.len(), 1);
    }

    #[test]
    fn two_candidate_modules_are_an_error_not_a_coin_flip() {
        let files = vec![
            file("a.js", 1),
            file("a_bg.wasm", 1),
            file("b.js", 1),
            file("b_bg.wasm", 1),
        ];
        let err = classify(&files, Framework::Leptos).expect_err("ambiguous");
        assert!(err.contains("exactly one entry module"), "{err}");
    }

    #[test]
    fn glue_missing_beside_the_module_is_an_error() {
        let files = vec![file("orphan_bg.wasm", 1)];
        let err = classify(&files, Framework::Leptos).expect_err("no glue");
        assert!(err.contains("orphan.js"), "{err}");
    }

    #[test]
    fn staging_is_conservative() {
        assert_eq!(stage_for("assets/FontManifest.json", 100), "critical");
        assert_eq!(stage_for("app.css", 100), "critical");
        assert_eq!(stage_for("canvaskit/skwasm.wasm", 900_000), "optional");
        assert_eq!(stage_for("canvaskit/huge.wasm", 40_000_000), "lazy");
        assert_eq!(stage_for("assets/help/onboarding.json", 100), "lazy");
    }

    #[test]
    fn wasm_gets_the_content_type_streaming_instantiation_requires() {
        assert_eq!(content_type_for("main.dart.wasm"), "application/wasm");
        assert_eq!(content_type_for("islands.js"), "text/javascript");
        assert_eq!(content_type_for("main.dart.mjs"), "text/javascript");
    }
}
