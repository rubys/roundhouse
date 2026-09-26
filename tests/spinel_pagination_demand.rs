//! geared_pagination's controller reopen, on the spinel tree, only when
//! the app paginates through it.
//!
//! `runtime/ruby/action_controller/pagination.rb` reopens
//! `ActionController::Base` to give `set_page_and_extract_portion_from`
//! its `@page`. lobsters never calls it and keeps its own page NUMBER in
//! `@page`, and spinel refuses the two as a class-layout conflict (an
//! `sp_Page *` in the base, an Integer in the subclass). So a tree whose
//! app never calls the method gets neither the file nor its require
//! (`project::apply_pagination_demand`); campfire, which does, keeps both.

use std::path::Path;

use roundhouse::ingest::ingest_app;
use roundhouse::project::{target_files, BuildTarget};

const REQUIRE: &str = "require_relative \"action_controller/pagination\"";

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn spinel_files(paginates: bool) -> Vec<(String, String)> {
    let fixture = roundhouse::fixtures::real_blog().to_path_buf();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-pagination-demand-{}-{paginates}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["app", "config", "db"] {
        copy_dir(&fixture.join(sub), &dir.join(sub));
    }
    if paginates {
        std::fs::write(
            dir.join("app/controllers/pages_controller.rb"),
            "class PagesController < ApplicationController\n  def index\n    set_page_and_extract_portion_from Article.all\n  end\nend\n",
        )
        .unwrap();
    }
    let app = ingest_app(&dir).expect("ingest");
    let files = target_files(&app, &dir, BuildTarget::Spinel).expect("spinel files");
    let _ = std::fs::remove_dir_all(&dir);
    files
}

fn action_controller(files: &[(String, String)]) -> &str {
    &files
        .iter()
        .find(|(p, _)| p == "runtime/action_controller.rb")
        .expect("runtime/action_controller.rb")
        .1
}

fn has(files: &[(String, String)], path: &str) -> bool {
    files.iter().any(|(p, _)| p == path)
}

#[test]
fn an_app_that_never_paginates_gets_no_page_reopen() {
    let files = spinel_files(false);
    assert!(!action_controller(&files).contains(REQUIRE));
    assert!(!has(&files, "runtime/action_controller/pagination.rb"));
}

#[test]
fn an_app_that_paginates_keeps_it() {
    let files = spinel_files(true);
    assert!(action_controller(&files).contains(REQUIRE));
    assert!(has(&files, "runtime/action_controller/pagination.rb"));
}
