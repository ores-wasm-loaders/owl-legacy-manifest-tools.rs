//! Building an `owl-manifest.json` from a scanned tree, and verifying one against a tree.
//!
//! Verification is fail-closed: a missing file, a changed digest, a mutable release path or
//! an incoherent budget is an error. The point is to catch these in CI, where the fix costs
//! a rerun, rather than in a browser, where it costs a visitor.

use std::collections::BTreeMap;

use crate::json::{parse, Json};
use crate::scan::{classify, stage_for, Framework, Role, ScannedFile};

pub const CONTRACT_VERSION: &str = "1.0.0";

#[derive(Debug, Clone)]
pub struct BuildSpec {
    pub app_id: String,
    pub release_id: String,
    pub framework: Framework,
    pub toolchain: String,
    pub base_url: String,
    pub requires_cross_origin_isolation: bool,
    pub max_concurrency: i64,
    pub furthest_stage: String,
    pub activation_mode: String,
    pub host_selector: Option<String>,
    pub islands: Vec<String>,
    pub routes: BTreeMap<String, String>,
}

fn entry_json(file: &ScannedFile, role: Role) -> Json {
    let mut map = BTreeMap::new();
    map.insert("role".into(), Json::Str(role.as_str().into()));
    map.insert("path".into(), Json::Str(file.path.clone()));
    map.insert("contentType".into(), Json::Str(file.content_type.clone()));
    map.insert("bytes".into(), Json::Int(file.bytes as i64));
    map.insert("sha256".into(), Json::Str(file.sha256.clone()));
    Json::Obj(map)
}

fn asset_json(file: &ScannedFile) -> Json {
    let mut map = BTreeMap::new();
    map.insert("path".into(), Json::Str(file.path.clone()));
    map.insert("contentType".into(), Json::Str(file.content_type.clone()));
    map.insert("bytes".into(), Json::Int(file.bytes as i64));
    map.insert("sha256".into(), Json::Str(file.sha256.clone()));
    map.insert(
        "stage".into(),
        Json::Str(stage_for(&file.path, file.bytes).into()),
    );
    Json::Obj(map)
}

/// Build the manifest for a scanned release.
pub fn build(files: &[ScannedFile], spec: &BuildSpec) -> Result<Json, String> {
    if !spec.base_url.ends_with('/') {
        return Err(format!(
            "base URL `{}` must end with a slash",
            spec.base_url
        ));
    }
    if !spec.base_url.contains(&spec.release_id) {
        return Err(format!(
            "base URL `{}` does not contain the release id `{}`: releases must be addressable immutably, so a later publish cannot shadow this one",
            spec.base_url, spec.release_id
        ));
    }
    if spec.framework == Framework::Flutter && spec.furthest_stage == "compile" {
        return Err(
            "flutter releases prepare fetch-only: its supported bootstrap owns compilation".into(),
        );
    }

    let (entries, assets) = classify(files, spec.framework)?;

    // The budget must cover everything preparation will actually reach, or preparation is
    // guaranteed to truncate — which looks like a mysterious slow start, not like a bug.
    let critical: u64 = entries
        .iter()
        .map(|(f, _)| f.bytes)
        .chain(
            assets
                .iter()
                .filter(|f| stage_for(&f.path, f.bytes) == "critical")
                .map(|f| f.bytes),
        )
        .sum();
    let optional: u64 = assets
        .iter()
        .filter(|f| stage_for(&f.path, f.bytes) == "optional")
        .map(|f| f.bytes)
        .sum();
    let max_bytes = critical + optional;

    let mut activation = BTreeMap::new();
    activation.insert("mode".into(), Json::Str(spec.activation_mode.clone()));
    if let Some(selector) = &spec.host_selector {
        activation.insert("hostSelector".into(), Json::Str(selector.clone()));
    }
    if !spec.islands.is_empty() {
        activation.insert(
            "islands".into(),
            Json::Arr(spec.islands.iter().cloned().map(Json::Str).collect()),
        );
    }
    if !spec.routes.is_empty() {
        activation.insert(
            "routes".into(),
            Json::Obj(
                spec.routes
                    .iter()
                    .map(|(k, v)| (k.clone(), Json::Str(v.clone())))
                    .collect(),
            ),
        );
    }

    let mut prepare = BTreeMap::new();
    prepare.insert("maxBytes".into(), Json::Int(max_bytes as i64));
    prepare.insert("maxConcurrency".into(), Json::Int(spec.max_concurrency));
    prepare.insert(
        "furthestStage".into(),
        Json::Str(spec.furthest_stage.clone()),
    );

    let mut root = BTreeMap::new();
    root.insert("contractVersion".into(), Json::Str(CONTRACT_VERSION.into()));
    root.insert("appId".into(), Json::Str(spec.app_id.clone()));
    root.insert("releaseId".into(), Json::Str(spec.release_id.clone()));
    root.insert(
        "framework".into(),
        Json::Str(spec.framework.as_str().into()),
    );
    root.insert("toolchain".into(), Json::Str(spec.toolchain.clone()));
    root.insert("baseUrl".into(), Json::Str(spec.base_url.clone()));
    root.insert(
        "requiresCrossOriginIsolation".into(),
        Json::Bool(spec.requires_cross_origin_isolation),
    );
    root.insert(
        "entrypoints".into(),
        Json::Arr(entries.iter().map(|(f, r)| entry_json(f, *r)).collect()),
    );
    root.insert(
        "assets".into(),
        Json::Arr(assets.iter().map(asset_json).collect()),
    );
    root.insert("prepare".into(), Json::Obj(prepare));
    root.insert("activation".into(), Json::Obj(activation));
    Ok(Json::Obj(root))
}

#[derive(Debug, Default)]
pub struct Verification {
    pub checked: usize,
    pub problems: Vec<String>,
}

impl Verification {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Verify a manifest against the tree it claims to describe.
pub fn verify(manifest_text: &str, files: &[ScannedFile]) -> Result<Verification, String> {
    let doc = parse(manifest_text).map_err(|e| format!("manifest is not valid JSON: {e}"))?;
    let mut report = Verification::default();

    let version = doc
        .get("contractVersion")
        .and_then(Json::as_str)
        .unwrap_or("");
    if version != CONTRACT_VERSION {
        report.problems.push(format!(
            "contractVersion is `{version}`, this tool speaks `{CONTRACT_VERSION}`"
        ));
    }
    let release_id = doc.get("releaseId").and_then(Json::as_str).unwrap_or("");
    let base_url = doc.get("baseUrl").and_then(Json::as_str).unwrap_or("");
    if release_id.is_empty() {
        report.problems.push("manifest has no releaseId".into());
    } else if !base_url.contains(release_id) {
        report.problems.push(format!(
            "baseUrl `{base_url}` does not contain releaseId `{release_id}`: the release is not immutably addressable"
        ));
    }

    let on_disk: BTreeMap<&str, &ScannedFile> =
        files.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut listed: Vec<String> = Vec::new();

    for (section, items) in [
        ("entrypoints", doc.get("entrypoints")),
        ("assets", doc.get("assets")),
    ] {
        let items = items.and_then(Json::as_array).unwrap_or(&[]);
        for item in items {
            let path = item.get("path").and_then(Json::as_str).unwrap_or_default();
            let digest = item
                .get("sha256")
                .and_then(Json::as_str)
                .unwrap_or_default();
            let bytes = item.get("bytes").and_then(Json::as_int).unwrap_or(-1);
            listed.push(path.to_string());
            report.checked += 1;
            match on_disk.get(path) {
                None => report.problems.push(format!(
                    "{section}: `{path}` is in the manifest but not in the build output"
                )),
                Some(file) => {
                    if file.sha256 != digest {
                        report.problems.push(format!(
                            "{section}: `{path}` digest {digest} does not match the file ({})",
                            file.sha256
                        ));
                    }
                    if file.bytes as i64 != bytes {
                        report.problems.push(format!(
                            "{section}: `{path}` says {bytes} bytes, the file is {}",
                            file.bytes
                        ));
                    }
                    if item
                        .get("contentType")
                        .and_then(Json::as_str)
                        .is_some_and(|declared| declared != file.content_type)
                    {
                        report.problems.push(format!(
                            "{section}: `{path}` is declared as {} but its extension implies {}",
                            item.get("contentType").and_then(Json::as_str).unwrap_or(""),
                            file.content_type
                        ));
                    }
                }
            }
        }
    }

    for file in files {
        if !listed.contains(&file.path) {
            report.problems.push(format!(
                "`{}` is in the build output but not in the manifest: an unlisted file can never be prepared",
                file.path
            ));
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{build, verify, BuildSpec};
    use crate::scan::{content_type_for, Framework, ScannedFile};
    use crate::sha256::sha256_hex;
    use std::collections::BTreeMap;

    fn file(path: &str, body: &str) -> ScannedFile {
        ScannedFile {
            path: path.into(),
            bytes: body.len() as u64,
            sha256: sha256_hex(body.as_bytes()),
            content_type: content_type_for(path),
        }
    }

    fn spec() -> BuildSpec {
        BuildSpec {
            app_id: "demo-web".into(),
            release_id: "2026.09.05-abc".into(),
            framework: Framework::Leptos,
            toolchain: "wasm-bindgen 0.2.95".into(),
            base_url: "/assets/releases/2026.09.05-abc/".into(),
            requires_cross_origin_isolation: false,
            max_concurrency: 3,
            furthest_stage: "compile".into(),
            activation_mode: "hydrate-islands".into(),
            host_selector: None,
            islands: vec!["Pricing".into()],
            routes: BTreeMap::new(),
        }
    }

    fn tree() -> Vec<ScannedFile> {
        vec![
            file("app.js", "glue"),
            file("app_bg.wasm", "module bytes"),
            file("app.css", "styles"),
        ]
    }

    #[test]
    fn a_generated_manifest_verifies_against_the_tree_it_came_from() {
        let files = tree();
        let manifest = build(&files, &spec()).expect("builds");
        let report = verify(&manifest.to_pretty(), &files).expect("verifies");
        assert!(report.ok(), "{:?}", report.problems);
        assert_eq!(report.checked, files.len());
    }

    #[test]
    fn generation_is_deterministic() {
        let files = tree();
        assert_eq!(
            build(&files, &spec()).expect("builds").to_pretty(),
            build(&files, &spec()).expect("builds").to_pretty()
        );
    }

    #[test]
    fn the_budget_covers_everything_preparation_will_reach() {
        let files = tree();
        let manifest = build(&files, &spec()).expect("builds");
        let max = manifest
            .get("prepare")
            .and_then(|p| p.get("maxBytes"))
            .and_then(crate::json::Json::as_int)
            .expect("maxBytes");
        let total: i64 = files.iter().map(|f| f.bytes as i64).sum();
        assert_eq!(max, total, "every critical and optional byte must fit");
    }

    #[test]
    fn a_mutable_release_path_is_refused() {
        let mut spec = spec();
        spec.base_url = "/assets/latest/".into();
        let err = build(&tree(), &spec).expect_err("should refuse");
        assert!(err.contains("immutably"), "{err}");
    }

    #[test]
    fn flutter_may_not_declare_the_compile_stage() {
        let mut spec = spec();
        spec.framework = Framework::Flutter;
        let err = build(&tree(), &spec).expect_err("should refuse");
        assert!(err.contains("fetch-only"), "{err}");
    }

    #[test]
    fn a_changed_file_fails_verification() {
        let files = tree();
        let manifest = build(&files, &spec()).expect("builds").to_pretty();
        let mut changed = files.clone();
        changed[1] = file("app_bg.wasm", "different module bytes");
        let report = verify(&manifest, &changed).expect("verifies");
        assert!(!report.ok());
        assert!(
            report.problems.iter().any(|p| p.contains("digest")),
            "{:?}",
            report.problems
        );
        assert!(
            report.problems.iter().any(|p| p.contains("bytes")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn a_file_added_after_generation_fails_verification() {
        let files = tree();
        let manifest = build(&files, &spec()).expect("builds").to_pretty();
        let mut extra = files.clone();
        extra.push(file("sneaky.js", "added later"));
        let report = verify(&manifest, &extra).expect("verifies");
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.contains("not in the manifest")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn a_missing_file_fails_verification() {
        let files = tree();
        let manifest = build(&files, &spec()).expect("builds").to_pretty();
        let report = verify(&manifest, &files[..1]).expect("verifies");
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.contains("not in the build output")),
            "{:?}",
            report.problems
        );
    }
}
