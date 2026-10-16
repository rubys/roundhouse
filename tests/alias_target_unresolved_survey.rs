//! CLI-level shape of an unresolved model `alias` under the real
//! `roundhouse` transpile entry point (`bin/roundhouse.rs`), not the
//! lowering function in isolation.
//!
//! `resolve_model_aliases` pushes its diagnostic from inside
//! `project::target_files`'s own `emit::diagnostics::scope` — the
//! ONLY place such a push is ever collected, since
//! `session::analyze_and_lower`'s own lowering pass (run earlier, to
//! build `analyze_diags`) isn't wrapped in that scope at all.
//! `attribute_analysis_gaps` used to run only over `analyze_diags`,
//! built BEFORE `target_files` runs — so it never saw this
//! diagnostic: `--survey` without `--allow-unsupported` still
//! hard-failed on an unresolved alias (an `Error`-severity diagnostic
//! in the SEPARATE `diags` collection still gates the exit code), and
//! the gap never reached the punch list, undercounting the app's
//! real gap total.

use std::path::{Path, PathBuf};
use std::process::Command;

fn app() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "roundhouse_alias_survey_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let files = [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        ),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  alias missing_thing nowhere_defined\nend\n",
        ),
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ];
    for (path, content) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create app directory");
        std::fs::write(path, content).expect("write app file");
    }
    root
}

fn roundhouse(args: &[&str], input: &Path) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(args)
        .arg(input)
        .arg("-o")
        .arg(input.join("out"))
        .output()
        .expect("spawn roundhouse");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn strict_mode_fails_on_an_unresolved_alias() {
    let root = app();
    let (code, err) = roundhouse(&["--target", "ruby"], &root);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("error[unsupported]: alias_target_unresolved"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// The regression: survey mode WITHOUT `--allow-unsupported` must
/// still exit 0 and must record the alias as exactly one gap in the
/// punch list — the same non-fatal, counted treatment the ingest-time
/// refusal had before this alias moved to lowering.
#[test]
fn survey_mode_without_allow_unsupported_records_exactly_one_gap_and_succeeds() {
    let root = app();
    let (code, err) = roundhouse(&["--target", "ruby", "--survey"], &root);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("── Survey: 1 ingest gap(s), 1 distinct kind(s) ──"),
        "expected exactly one recorded gap: {err}"
    );
    assert!(err.contains("alias_target_unresolved"), "{err}");
    assert!(root.join("out").is_dir(), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Same shape with `--allow-unsupported` too (the combination the
/// Mastodon pipeline actually runs): still exactly one gap, not a
/// silently-dropped warning.
#[test]
fn survey_mode_with_allow_unsupported_also_records_exactly_one_gap() {
    let root = app();
    let (code, err) = roundhouse(&["--target", "ruby", "--survey", "--allow-unsupported"], &root);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("── Survey: 1 ingest gap(s), 1 distinct kind(s) ──"),
        "expected exactly one recorded gap: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The attribution count printed alongside the punch list
/// ("N diagnostic(s) attributed to them") tallied only `analyze_diags`
/// — the diagnostics from `session::analyze_and_lower`. This alias's
/// own `alias_target_unresolved` diagnostic is pushed from
/// `resolve_model_aliases` *inside* `project::target_files`'s own
/// `emit::diagnostics::scope`, which runs AFTER `analyze_and_lower`
/// and is never folded back into `analyze_diags` — so this is a case
/// where the attributed note comes from the emit phase alone, and the
/// old count reported 0 attributed notes for a run that plainly
/// attributed one (the gap line right above it says so). The report
/// line must count the emit-phase `diags` too.
#[test]
fn the_attribution_count_includes_a_note_attributed_only_in_the_emit_phase() {
    let root = app();
    let (code, err) = roundhouse(&["--target", "ruby", "--survey"], &root);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("1 ingest gap(s) recorded; 1 diagnostic(s) attributed to them"),
        "expected the one alias gap to count as one attributed note, not 0: {err}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
