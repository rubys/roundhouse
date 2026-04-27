//! `build-site` — assemble the GitHub Pages site.
//!
//! For each target language, ingest the real-blog fixture, run the
//! analyzer, emit the project into memory, and write a JSON manifest
//! (`{ language, files: [{ path, content }] }`) to
//! `_site/browse/<lang>.json`. For the "ruby" tab, walk the fixture
//! itself and include its source files.
//!
//! Static assets (landing page + browse subpath) are copied from
//! `site/` into `_site/`.
//!
//! Usage:
//!
//!     cargo run --bin build-site -- [FIXTURE] [OUTDIR]
//!
//! Defaults: `fixtures/real-blog` and `_site`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::{self, EmittedFile};
use roundhouse::ingest::ingest_app;

const TARGETS: &[&str] = &[
    "ruby", "spinel", "crystal", "elixir", "go", "python", "rust", "typescript",
];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let fixture = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("fixtures/real-blog"));
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("_site"));

    match run(&fixture, &out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("build-site: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(fixture: &Path, out: &Path) -> Result<(), String> {
    if out.exists() {
        fs::remove_dir_all(out).map_err(|e| format!("clean {}: {e}", out.display()))?;
    }
    fs::create_dir_all(out.join("browse"))
        .map_err(|e| format!("mkdir {}: {e}", out.display()))?;

    copy_site_assets(out)?;

    let mut app = ingest_app(fixture)
        .map_err(|e| format!("ingest {}: {e}", fixture.display()))?;
    Analyzer::new(&app).analyze(&mut app);

    for target in TARGETS {
        let manifest = match *target {
            "ruby" => ruby_manifest(fixture)?,
            "spinel" => build_manifest("spinel", emit::ruby::emit_spinel(&app)),
            "crystal" => build_manifest("crystal", emit::crystal::emit(&app)),
            "elixir" => build_manifest("elixir", emit::elixir::emit(&app)),
            "go" => build_manifest("go", emit::go::emit(&app)),
            "python" => build_manifest("python", emit::python::emit(&app)),
            "rust" => build_manifest("rust", emit::rust::emit(&app)),
            "typescript" => build_manifest("typescript", emit::typescript::emit(&app)),
            _ => unreachable!(),
        };
        let path = out.join("browse").join(format!("{target}.json"));
        fs::write(&path, &manifest)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        eprintln!("wrote {}", path.display());
    }

    Ok(())
}

fn copy_site_assets(out: &Path) -> Result<(), String> {
    let site = PathBuf::from("site");
    if !site.exists() {
        return Err(format!("missing {}/ (static assets)", site.display()));
    }
    copy_tree(&site, out)
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    for entry in fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ty = entry.file_type().map_err(|e| format!("stat: {e}"))?;
        if ty.is_dir() {
            fs::create_dir_all(&dst_path)
                .map_err(|e| format!("mkdir {}: {e}", dst_path.display()))?;
            copy_tree(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {} → {}: {e}", src_path.display(), dst_path.display()))?;
        }
    }
    Ok(())
}

fn build_manifest(language: &str, files: Vec<EmittedFile>) -> String {
    let mut entries: Vec<(String, String)> = files
        .into_iter()
        .map(|f| (f.path.to_string_lossy().into_owned(), f.content))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    write_manifest_json(language, &entries)
}

fn ruby_manifest(fixture: &Path) -> Result<String, String> {
    let mut files: Vec<(String, String)> = Vec::new();
    walk_ruby(fixture, fixture, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(write_manifest_json("ruby", &files))
}

fn walk_ruby(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let ty = entry.file_type().map_err(|e| format!("stat: {e}"))?;
        if ty.is_dir() {
            walk_ruby(root, &path, files)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| format!("strip prefix: {e}"))?;
            let content = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if content.contains('\0') {
                continue;
            }
            files.push((rel.to_string_lossy().into_owned(), content));
        }
    }
    Ok(())
}

fn write_manifest_json(language: &str, files: &[(String, String)]) -> String {
    #[derive(serde::Serialize)]
    struct File<'a> {
        path: &'a str,
        content: &'a str,
    }
    #[derive(serde::Serialize)]
    struct Manifest<'a> {
        language: &'a str,
        files: Vec<File<'a>>,
    }
    let manifest = Manifest {
        language,
        files: files
            .iter()
            .map(|(p, c)| File { path: p, content: c })
            .collect(),
    };
    serde_json::to_string(&manifest).expect("serialize manifest")
}
