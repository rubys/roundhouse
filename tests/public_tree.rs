//! Everything under `public/` is carried, at any depth.
//!
//! Rails serves `public/` as it is on disk (ActionDispatch::Static).
//! hindmost's terminal page loads xterm from `public/vendor/xterm/`, and
//! the emit dropped that directory: the walker skipped any directory
//! named `vendor` (meant for bundled gems), at every depth. The page
//! rendered and every script under it answered 404.

use std::fs;
use std::path::PathBuf;

fn app_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rh-public-tree-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let files: &[(&str, &str)] = &[
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("public/robots.txt", "User-agent: *\n"),
        ("public/vendor/xterm/xterm.min.js", "/* xterm */\n"),
        ("public/vendor/xterm/xterm.min.css", ".xterm{}\n"),
        ("public/node_modules/x/index.js", "// served too\n"),
    ];
    for (path, content) in files {
        let p = dir.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
    dir
}

#[test]
fn nested_public_directories_are_emitted_as_rails_serves_them() {
    let dir = app_dir();
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, &dir).expect("spinel tree");
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    for want in [
        "public/robots.txt",
        "public/vendor/xterm/xterm.min.js",
        "public/vendor/xterm/xterm.min.css",
        "public/node_modules/x/index.js",
    ] {
        assert!(paths.contains(&want), "{want} missing from the emitted tree");
    }
    let _ = fs::remove_dir_all(&dir);
}
