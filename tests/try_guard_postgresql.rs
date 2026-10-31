//! PostgreSQL: try guard postgresql, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\n\
                            test:\n  <<: *default\n  database: rh_try_guard_probe\n";

const DEFINERS: &str = "class T1\n  def m = 1\nend\n\nclass T2\n  def m = 2\nend\n\n\
                        class T3\n  def m = 3\nend\n\nclass T4\n  def m = 4\nend\n";

const TRY_USER: &str = "class TryUser\n  def self.call(x)\n    x.try(:m)\n  end\n\n  \
                        def self.bang(x)\n    x.try!(:m)\n  end\nend\n";

fn emit(postgresql: bool) -> Vec<(String, String)> {
    let mut files: Vec<(&str, &str)> = vec![
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/t.rb", DEFINERS),
        ("app/models/try_user.rb", TRY_USER),
    ];
    if postgresql {
        files.push(("roundhouse.yml", ""));
        files.push(("config/database.yml", DATABASE_YML));
        files.push(("db/structure.sql", ""));
    } else {
        files.push(("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"));
    }
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .into_iter()
        .chain(ruby::emit_library(&app))
        .map(|f| (f.path.display().to_string(), f.content))
        .collect()
}

fn joined(files: &[(String, String)]) -> String {
    files.iter().map(|(_, c)| c.as_str()).collect::<Vec<_>>().join("\n")
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_postgresql_app_keeps_try_past_three_definers() {
    let files = emit(true);
    let src = joined(&files);
    assert!(src.contains("x.try(:m)"), "the PostgreSQL tree has Tryable:\n{src}");
    assert!(!src.contains("x && x.m"), "the nil guard calls `m` on any non-nil:\n{src}");
    assert!(src.contains("x.try!(:m)"), "try! is Tryable's too:\n{src}");
}

/// The spinel grounding is unchanged for an app without PostgreSQL.
#[test]
fn an_app_without_postgresql_still_grounds_try() {
    let src = joined(&emit(false));
    assert!(src.contains("x && x.m"), "four definers is past MAX_ARMS:\n{src}");
    assert!(!src.contains("x.try(:m)"), "{src}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_kept_try_answers_like_rails_on_the_runtime() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = emit(true);
    let dir = std::env::temp_dir().join(format!("rh-try-guard-pg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut script = format!(
        "require {:?}\n",
        root.join("runtime/spinel/active_support_tryable.rb").display().to_string()
    );
    for (path, content) in files.iter().filter(|(p, _)| p.starts_with("app/models/") && p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content).unwrap();
        script.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    script.push_str(
        "p TryUser.call(T1.new), TryUser.call(T4.new), TryUser.call(Object.new), TryUser.call(nil), \
         TryUser.call(RuntimeError.new(\"x\"))\n\
         p TryUser.bang(T1.new), (TryUser.bang(Object.new) rescue $!.class), TryUser.bang(nil)\n",
    );
    let out = Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}\n{script}");
    assert_eq!(stdout, "1\n4\nnil\nnil\nnil\n1\nNoMethodError\nnil\n", "{stderr}\n{script}");
}
