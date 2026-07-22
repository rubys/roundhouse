//! Jeremy's #67 round-trip equivalence gate for the Rails → Roda/Sequel
//! conversion: ingest the ORIGINAL Rails app and the CONVERTED app and
//! check they produce the same IR ("One possibility for testing the
//! convertor … would be to ingest both the original application and the
//! converted application with roundhouse and check whether they result
//! in the same IR").
//!
//! The comparison is per-surface, with the deliberate conversion deltas
//! documented at each site:
//!
//!   * routes — same REST surface; param NAMES normalize (Rails spells
//!     the nested id `:article_id`, the Roda tree binds positionally),
//!     and the root route compares by presence (Rails serves the index
//!     at `/`, the conversion redirects to the canonical `/articles` —
//!     the Jeremy-reviewed exemplar idiom).
//!   * schema — tables/columns/indexes; the Sequel side's
//!     `foreign_key` columns ingest as `Reference` where the Rails
//!     schema spells `Integer` (same fact, different carrier), and FK
//!     referential actions differ by design (`dependent: :destroy`
//!     moves to `on_delete: :cascade`).
//!   * models — association + validation shapes converge; the Rails
//!     side's Turbo-broadcast declarations have no Roda equivalent and
//!     ride the converted tree as ROUNDHOUSE-TODO comments, so they are
//!     absent from the re-ingest by design.
//!
//! The `#[ignore]`d companion runs the exemplar's own behavioral suite
//! (fixtures/roda-blog/test/blog_test.rb — "a transpiled version of
//! this app must pass it unchanged") against the converted tree.

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::lower::flatten_routes;
use roundhouse::App;

fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-roda-{tag}"))
}

/// Ingest-shape Rails app (NO analyze_and_lower — the converter's
/// contract; lowering rewrites controller bodies into runtime
/// vocabulary, see `emit::roda`).
fn ingest_rails() -> App {
    roundhouse::ingest::ingest_app(Path::new("fixtures/real-blog")).expect("ingest real-blog")
}

fn write_conversion(scratch: &Path) {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).expect("clean scratch");
    }
    let rails = ingest_rails();
    let files = roundhouse::emit::roda::emit(&rails, Path::new("fixtures/real-blog"));
    for f in files {
        let out = scratch.join(&f.path);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&out, &f.content).expect("write");
    }
}

/// (method, path-with-anonymous-params, action) rows, sorted. Controller
/// names ride along for every non-root row.
fn route_rows(app: &App) -> Vec<(String, String, String, String)> {
    let mut rows: Vec<(String, String, String, String)> = flatten_routes(app)
        .into_iter()
        .filter(|r| r.path != "/")
        .map(|r| {
            let norm_path = r
                .path
                .split('/')
                .map(|seg| if seg.starts_with(':') { ":p" } else { seg })
                .collect::<Vec<_>>()
                .join("/");
            (
                format!("{:?}", r.method),
                norm_path,
                r.controller.0.to_string(),
                r.action.to_string(),
            )
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

#[test]
fn conversion_roundtrips_through_ingest() {
    let scratch = scratch_dir("roundtrip");
    write_conversion(&scratch);

    let rails = ingest_rails();
    let roda =
        roundhouse::ingest::ingest_app(&scratch).expect("re-ingest the converted app");

    // ── Routes ─────────────────────────────────────────────────────
    assert_eq!(
        route_rows(&rails),
        route_rows(&roda),
        "REST route surface must round-trip"
    );
    // Both serve `/` (Rails: index; conversion: canonical redirect).
    assert!(flatten_routes(&rails).iter().any(|r| r.path == "/"));
    assert!(flatten_routes(&roda).iter().any(|r| r.path == "/"));

    // ── Schema ─────────────────────────────────────────────────────
    let rails_tables: Vec<&str> =
        rails.schema.tables.keys().map(|k| k.as_str()).collect();
    let roda_tables: Vec<&str> = roda.schema.tables.keys().map(|k| k.as_str()).collect();
    assert_eq!(rails_tables, roda_tables);
    for (name, rt) in &rails.schema.tables {
        let ot = &roda.schema.tables[name];
        let norm = |t: &roundhouse::schema::Table| -> Vec<(String, String, bool, bool)> {
            t.columns
                .iter()
                .map(|c| {
                    // `foreign_key :article_id` ingests as Reference on
                    // the Sequel side; the Rails schema spells Integer.
                    // Same column, two carriers of the same FK fact.
                    // Likewise Rails' implicit BigInt pk vs Sequel's
                    // `primary_key :id` (Integer) — in SQLite both are
                    // the same 64-bit INTEGER PRIMARY KEY rowid.
                    let ty = match &c.col_type {
                        roundhouse::schema::ColumnType::Reference { .. } => {
                            "Integer".to_string()
                        }
                        roundhouse::schema::ColumnType::BigInt if c.primary_key => {
                            "Integer".to_string()
                        }
                        other => format!("{other:?}"),
                    };
                    (c.name.to_string(), ty, c.nullable, c.primary_key)
                })
                .collect()
        };
        assert_eq!(norm(rt), norm(ot), "columns of {name} must round-trip");
        let fk_pairs = |t: &roundhouse::schema::Table| -> Vec<(String, String, String)> {
            t.foreign_keys
                .iter()
                .map(|fk| {
                    (
                        fk.from_column.to_string(),
                        fk.to_table.0.to_string(),
                        fk.to_column.to_string(),
                    )
                })
                .collect()
        };
        // Referential ACTIONS deliberately differ (dependent: :destroy
        // → on_delete: :cascade); the FK edges themselves must match.
        assert_eq!(fk_pairs(rt), fk_pairs(ot), "FK edges of {name} must round-trip");
    }

    // ── Models ─────────────────────────────────────────────────────
    let model_names = |app: &App| -> Vec<String> {
        let mut names: Vec<String> =
            app.models.iter().map(|m| m.name.0.to_string()).collect();
        names.sort();
        names
    };
    assert_eq!(model_names(&rails), model_names(&roda));
    for rm in &rails.models {
        let om = roda
            .models
            .iter()
            .find(|m| m.name == rm.name)
            .expect("model present after round-trip");
        let assocs = |m: &roundhouse::dialect::Model| -> Vec<String> {
            let mut a: Vec<String> = m
                .associations()
                .map(|a| format!("{}:{}", a.name(), assoc_kind(a)))
                .collect();
            a.sort();
            a
        };
        assert_eq!(assocs(rm), assocs(om), "associations of {} must round-trip", rm.name.0);
        let validations = |m: &roundhouse::dialect::Model| -> Vec<String> {
            let mut v: Vec<String> = m
                .validations()
                .flat_map(|v| {
                    v.rules.iter().map(move |r| format!("{}:{r:?}", v.attribute))
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            validations(rm),
            validations(om),
            "validations of {} must round-trip",
            rm.name.0
        );
    }
}

fn assoc_kind(a: &roundhouse::dialect::Association) -> &'static str {
    use roundhouse::dialect::Association::*;
    match a {
        BelongsTo { .. } => "belongs_to",
        HasMany { .. } => "has_many",
        HasOne { .. } => "has_one",
        HasAndBelongsToMany { .. } => "habtm",
    }
}

/// Behavioral gate: the exemplar's own 19-check suite against the
/// converted tree, on the real gems. Two checks are EXPECTED to fail —
/// both are genuine behavioral divergences between the two hand-written
/// fixtures, which the converted app resolves in the RAILS app's favor
/// (it is the conversion source):
///
///   * `test_create_invalid_article_rerenders_new_with_errors` — the
///     exemplar sets a custom min-length message ("must be at least 10
///     characters"); the Rails model has no custom message, so the
///     conversion carries Sequel's default text.
///   * `test_show_renders_article_and_comments` — the exemplar orders
///     comments newest-first (`order: Sequel.desc(:created_at)`); the
///     Rails association is unordered (insertion order).
///
/// Any OTHER failure is a converter regression. Ignored: needs bundler
/// + network (same posture as roda_blog_transpiled_oracle_passes).
#[test]
#[ignore]
fn converted_app_passes_exemplar_oracle_modulo_known_divergences() {
    let scratch = scratch_dir("oracle");
    write_conversion(&scratch);

    // The exemplar suite drives `Blog.freeze.app`; the converted class
    // is `App`. One-token rename, all 19 assertions unchanged.
    let suite = std::fs::read_to_string("fixtures/roda-blog/test/blog_test.rb")
        .expect("exemplar suite");
    let renamed = suite.replace("Blog.freeze.app", "App.freeze.app");
    std::fs::create_dir_all(scratch.join("test")).expect("mkdir test");
    std::fs::write(scratch.join("test/blog_test.rb"), renamed).expect("write suite");

    let gemfile = scratch.join("Gemfile");
    let install = Command::new("bundle")
        .env("BUNDLE_GEMFILE", &gemfile)
        .arg("install")
        .arg("--quiet")
        .current_dir(&scratch)
        .output()
        .expect("spawn bundle install");
    assert!(
        install.status.success(),
        "bundle install failed\n{}\n{}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    let output = Command::new("bundle")
        .env("BUNDLE_GEMFILE", &gemfile)
        .args(["exec", "ruby", "test/blog_test.rb"])
        .current_dir(&scratch)
        .output()
        .expect("spawn suite");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("19 runs"),
        "expected all 19 exemplar checks to run\n{stdout}"
    );
    let known = [
        "test_create_invalid_article_rerenders_new_with_errors",
        "test_show_renders_article_and_comments",
    ];
    let failed: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.split("BlogTest#").nth(1))
        .map(|l| l.split_whitespace().next().unwrap_or(""))
        .collect();
    let unexpected: Vec<&&str> =
        failed.iter().filter(|f| !known.contains(&f.trim_end_matches(':'))).collect();
    assert!(
        unexpected.is_empty(),
        "converter regression — unexpected oracle failures: {unexpected:?}\n{stdout}"
    );
    for k in known {
        assert!(
            failed.iter().any(|f| f.trim_end_matches(':') == k),
            "known divergence {k} did not fail — if the fixtures were aligned, \
             remove it from the known list\n{stdout}"
        );
    }
}
