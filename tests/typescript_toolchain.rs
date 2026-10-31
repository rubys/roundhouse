//! TypeScript toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted TypeScript project for a fixture into a
//! scratch directory, runs `npx tsc --noEmit` against it, asserts
//! zero errors. `--noEmit` performs parse + type-check without
//! writing JS output — the TS equivalent of `cargo check` /
//! `crystal build --no-codegen`.
//!
//! Marked `#[ignore]` because the first run on a machine without a
//! cached TypeScript install can take ~30s while npx pulls the
//! package. Run explicitly:
//!
//!     cargo test --test typescript_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "support/memoized_fields.rs"]
mod memoized_fields;
#[path = "support/app_class_base.rs"]
mod app_class_base;
#[path = "support/parenless_calls.rs"]
mod parenless_calls;
#[path = "support/model_class_methods.rs"]
mod model_class_methods;
#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/nested_classes.rs"]
mod nested_classes;
#[path = "support/array_methods.rs"]
mod array_methods;
#[path = "support/truthiness.rs"]
mod truthiness;
#[path = "support/class_constants.rs"]
mod class_constants;
#[path = "support/i18n_locales_app.rs"]
#[allow(dead_code)]
mod i18n_locales_app;
#[path = "support/record_slice_app.rs"]
#[allow(dead_code)]
mod record_slice_app;

use roundhouse::emit::typescript;
use roundhouse::ingest::ingest_app;

fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-ts-check-{tag}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    // Emit from the post-analyze lowered IR, as `roundhouse --target typescript` does
    roundhouse::session::analyze_and_lower(&mut app);
    let files = typescript::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn generate_project_with_profile(
    fixture_path: &Path,
    out: &Path,
    profile: &roundhouse::profile::DeploymentProfile,
) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    // Emit from the post-analyze lowered IR, as `roundhouse --target typescript` does
    roundhouse::session::analyze_and_lower(&mut app);
    let files = typescript::emit_with_profile(&app, profile);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn assert_tsc_passes(fixture: &str, scratch: &Path) {
    // Install declared devDependencies (`@types/node`) so tsc can
    // resolve `node:test` / `node:assert/strict` in the emitted specs.
    let install = Command::new("npm")
        .arg("install")
        .arg("--silent")
        .arg("--no-audit")
        .arg("--no-fund")
        .current_dir(scratch)
        .output()
        .expect("run npm install");
    assert!(
        install.status.success(),
        "npm install failed for {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    // Invoke the locally-installed tsc directly — `npx tsc` resolves
    // to a typosquatting-prevention stub, and `npx --package=` can
    // get confused by the node_modules we just populated.
    let output = Command::new("./node_modules/.bin/tsc")
        .arg("-p")
        .arg(".")
        .arg("--noEmit")
        .current_dir(scratch)
        .output()
        .expect("run tsc");

    assert!(
        output.status.success(),
        "tsc failed on emitted {fixture} project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// `Post.find(params[:id])`, the pre-Rails-8 spelling of `find(params.
/// expect(:id))`. Analysis is clean, so the emitted project must compile:
/// it read `this.params.get("id")` (the runtime's params is a Record, not
/// a class with `get`) and then handed `find(id: number)` a `ParamValue`.
/// The coercion is the TS emitter's: the Ruby family's casting finder takes
/// the raw String (tests/primary_key_casting.rs).
#[test]
#[ignore]
fn real_blog_find_by_params_index_tsc_passes() {
    let (emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "Article.find(params.expect(:id))",
            "Article.find(params[:id])",
        )
        .emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let controller =
        std::fs::read_to_string(emitted.join("app/controllers/articles_controller.ts")).expect("emitted controller");
    assert!(!controller.contains("this.params.get("), "{controller}");
    assert_tsc_passes("real-blog with params[:id]", &emitted);
}

/// `find(params[:id])` beside a row whose id is 0: a blank, absent or
/// non-String id finds nothing (RecordNotFound) rather than row 0, which
/// `Number("")` would answer; a numeric String finds its row.
#[test]
#[ignore]
fn find_by_params_index_rejects_a_blank_id_beside_row_zero() {
    let (emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "Article.find(params.expect(:id))",
            "Article.find(params[:id])",
        )
        .emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        r#"import Database from "better-sqlite3";
import { installDb } from "./src/juntos.js";
import { Db } from "./src/db.js";
import { Schema } from "./src/schema.js";
import { ArticlesController } from "./app/controllers/articles_controller.js";
const db = new Database(":memory:");
for (const stmt of Schema.statements()) db.exec(stmt);
installDb(db);
Db.install(db);
db.exec("INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (0, 'zero', 'b', '2026-01-01 00:00:00', '2026-01-01 00:00:00'), (31, 'thirty-one', 'b', '2026-01-01 00:00:00', '2026-01-01 00:00:00')");
const cases: [string, Record<string, any>][] = [["blank", { id: "" }], ["space", { id: " " }], ["missing", {}], ["array", { id: ["0"] }], ["abc", { id: "abc" }], ["zero", { id: "0" }], ["31", { id: "31" }]];
const out: string[] = [];
for (const [label, params] of cases) {
  const c: any = new ArticlesController();
  c.params = params;
  try { c.show(); } catch (e: any) { if (e.constructor.name === "RecordNotFound") { out.push(`${label}=RecordNotFound`); continue; } }
  out.push(`${label}=${c.article?.title}`);
}
console.log(out.join(","));
"#,
    );
    assert_eq!(
        out,
        "blank=RecordNotFound,space=RecordNotFound,missing=RecordNotFound,array=RecordNotFound,abc=RecordNotFound,zero=zero,31=thirty-one"
    );
}

/// The libsql adapter's `escape_int` beside a row whose id is 0: a NaN
/// key (a blank or non-numeric request id) finds nothing, as on the
/// better-sqlite3 adapter, rather than row 0.
#[test]
#[ignore]
fn libsql_find_of_nan_finds_nothing_beside_row_zero() {
    use roundhouse::profile::DeploymentProfile;

    let scratch = scratch_dir("real-blog-libsql-find-nan");
    generate_project_with_profile(roundhouse::fixtures::real_blog(), &scratch, &DeploymentProfile::node_async());
    let out = run_under_tsx(
        &scratch,
        r#"import { setupTestDb } from "./src/juntos.js";
import { Schema } from "./src/schema.js";
import { Db } from "./src/db.js";
import { Article } from "./app/models/article.js";
await setupTestDb(Schema.statements().join(";"));
await Db.exec("INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (0, 'zero', 'b', '2026-01-01 00:00:00', '2026-01-01 00:00:00')");
const out: string[] = [];
for (const id of [NaN, 0]) {
  try { out.push(`${id}=${(await Article.find(id) as any).title}`); }
  catch (e: any) { out.push(`${id}=${e.constructor.name}`); }
}
console.log(out.join(","));
"#,
    );
    assert_eq!(out, "NaN=RecordNotFound,0=zero");
}

/// A class that is not a model keeps its `find`'s argument: only a class
/// reaching `ActiveRecord::Base` has the numeric key the coercion assumes.
#[test]
#[ignore]
fn non_model_find_keeps_its_params_argument() {
    let (emitted, errors) = emit_and_run::real_blog()
        .write(
            "app/models/shelf.rb",
            "class Shelf\n  def self.find(name)\n    name.to_s.upcase\n  end\nend\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "Article.find(params.expect(:id))",
            "Article.find(params[:id])\n      @shelf = Shelf.find(params[:id])",
        )
        .emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let controller =
        std::fs::read_to_string(emitted.join("app/controllers/articles_controller.ts")).expect("emitted controller");
    assert!(controller.contains("Shelf.find(this.params[\"id\"])"), "{controller}");
    assert!(controller.contains("Article.find(Number(typeof this.params[\"id\"] === \"string\""), "{controller}");
    assert_tsc_passes("real-blog with a non-model find", &emitted);
}

/// Diagnostic test: emit transpiled_blog and dump tsc errors so we
/// know what the body-walker still needs to fix. Does NOT assert
/// success — purely an inspection helper.
#[test]
#[ignore]
fn dump_transpiled_blog_tsc_errors() {
    let fixture = Path::new("runtime/ruby/test/fixtures/transpiled_blog");
    let scratch = scratch_dir("transpiled-blog-tsc-dump");
    generate_project(fixture, &scratch);

    let install = Command::new("npm")
        .arg("install")
        .arg("--silent")
        .arg("--no-audit")
        .arg("--no-fund")
        .current_dir(&scratch)
        .output()
        .expect("run npm install");
    if !install.status.success() {
        eprintln!(
            "npm install failed:\n{}",
            String::from_utf8_lossy(&install.stderr)
        );
        return;
    }

    let output = Command::new("./node_modules/.bin/tsc")
        .arg("-p")
        .arg(".")
        .arg("--noEmit")
        .current_dir(&scratch)
        .output()
        .expect("run tsc");

    println!("=== tsc exit status: {} ===", output.status);
    println!("=== stdout ===\n{}", String::from_utf8_lossy(&output.stdout));
    println!("=== stderr ===\n{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore]
fn real_blog_tsc_passes() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-tsc");
    generate_project(fixture, &scratch);
    assert_tsc_passes("real-blog", &scratch);
}

/// libsql (`node-async`) profile: emit real-blog with async coloring
/// active, type-check the result. Forcing function for the libsql
/// runtime + async-emit pipeline — every regression in the
/// runtime↔app propagation handshake or the emit-time await-wrap
/// filters surfaces here as a tsc error.
#[test]
#[ignore]
fn real_blog_libsql_tsc_passes() {
    use roundhouse::profile::DeploymentProfile;

    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-libsql-tsc");
    generate_project_with_profile(fixture, &scratch, &DeploymentProfile::node_async());
    assert_tsc_passes("real-blog (libsql)", &scratch);
}

fn assert_node_test_passes(fixture: &str, scratch: &Path) {
    let install = Command::new("npm")
        .arg("install")
        .arg("--silent")
        .arg("--no-audit")
        .arg("--no-fund")
        .current_dir(scratch)
        .output()
        .expect("run npm install");
    assert!(
        install.status.success(),
        "npm install failed for {fixture} node test at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr),
    );

    // The emitted `npm test` script is the suite (recursive find,
    // fail closed when nothing matches). A second finder here is how
    // the harness and package.json drifted.
    let output = Command::new("npm")
        .arg("test")
        .current_dir(scratch)
        .output()
        .expect("run node --test via tsx");

    assert!(
        output.status.success(),
        "node --test failed on emitted {fixture} at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_node_test_passes() {
    // Phase 2 forcing function: emit real-blog, run `node:test` via
    // tsx (for TS transpile) against the emitted spec files, assert
    // zero failures. Mirrors the Rust/Crystal Phase 2 bar —
    // Phase-3-dependent tests are marked `test.skip(...)`.
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-node");
    generate_project(fixture, &scratch);
    assert_node_test_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_libsql_node_test_passes() {
    // libsql (`node-async`) profile: same forcing function as
    // `real_blog_node_test_passes` but exercises the async-coloring
    // emit path AND the libsql adapter at runtime. Every test
    // method awaits its HTTP-style helpers (`this.get(...)` etc.)
    // and AR calls (`Article.find(...)`); fixtures load through
    // `await _fixtures_load_bang()` which executes inserts on a
    // libsql in-memory client. Catches regressions in (a) the
    // runtime↔app propagation handshake, (b) the test-runtime
    // async surface (minitest.ts get/post/dispatch), and (c) the
    // libsql adapter's actual SQL execution.
    use roundhouse::profile::DeploymentProfile;

    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-libsql-node");
    generate_project_with_profile(fixture, &scratch, &DeploymentProfile::node_async());
    assert_node_test_passes("real-blog (libsql)", &scratch);
}

/// Install the emitted project's packages, run `script` under tsx from its
/// root, and answer what it printed.
fn run_under_tsx(emitted: &Path, script: &str) -> String {
    let install = Command::new("npm")
        .args(["install", "--silent", "--no-audit", "--no-fund"])
        .current_dir(emitted)
        .output()
        .expect("run npm install");
    assert!(install.status.success(), "{}", String::from_utf8_lossy(&install.stderr));
    std::fs::write(emitted.join("check.ts"), script).unwrap();
    let run = Command::new("npx")
        .args(["tsx", "check.ts"])
        .current_dir(emitted)
        .output()
        .expect("run tsx");
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    String::from_utf8_lossy(&run.stdout).trim().to_string()
}

#[test]
#[ignore]
fn real_blog_memoized_fields_run_under_tsx() {
    // Readers memoizing a literal answer it, on a plain class's two sides
    // and on a model, as CRuby does (#1238); the app compiles.
    let (emitted, errors) = memoized_fields::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { Meter } from \"./app/models/meter.js\";\nimport { Comment } from \"./app/models/comment.js\";\nconst m = new Meter();\nconsole.log(JSON.stringify([Meter.label, m.name, m.items, new Comment().tone]));\n",
    );
    assert_eq!(out, memoized_fields::SUMMARY);
    std::fs::remove_file(emitted.join("check.ts")).unwrap();
    assert_tsc_passes("real-blog with memoized readers", &emitted);
}

#[test]
#[ignore]
fn real_blog_app_class_named_like_the_alias_runs_under_tsx() {
    // An app `ActiveRecordBase` beside an app `Base`: the runtime base
    // takes another alias, and the app compiles and answers as CRuby.
    let (emitted, errors) = app_class_base::alias_taken_overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { Gadget } from \"./app/models/gadget.js\";\nconsole.log(Gadget.both());\n",
    );
    assert_eq!(out, app_class_base::BOTH);
    std::fs::remove_file(emitted.join("check.ts")).unwrap();
    assert_tsc_passes("real-blog with app Base and ActiveRecordBase", &emitted);
}

#[test]
#[ignore]
fn real_blog_app_class_base_runs_under_tsx() {
    // A class extending the app's own `Base` reads its class method and
    // constant as CRuby does, and the app, whose records and controllers
    // extend the runtime's base classes under aliases, compiles (#1228).
    let (emitted, errors) = app_class_base::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { Widget } from \"./app/models/widget.js\";\nconsole.log([Widget.label(), Widget.LABEL].join(\",\"));\n",
    );
    assert_eq!(out, app_class_base::SUMMARY);
    std::fs::remove_file(emitted.join("check.ts")).unwrap();
    assert_tsc_passes("real-blog with an app Base", &emitted);
}

#[test]
#[ignore]
fn real_blog_parenless_calls_run_under_tsx() {
    // A zero-argument `def` called without parens answers its value on a
    // plain class, from a subclass's class method and from a model's
    // class method, as CRuby does (#1229).
    let (emitted, errors) = parenless_calls::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { Meter } from \"./app/models/meter.js\";\nimport { Widget } from \"./app/models/widget.js\";\nimport { Comment } from \"./app/models/comment.js\";\nimport { Gauge } from \"./app/models/gauge.js\";\nconsole.log([Meter.a(), Meter.b(), Meter.c(), new Meter().e(), Widget.shout(), Comment.tags(), Gauge.show(), new Gauge().both()].join(\",\"));\n",
    );
    assert_eq!(out, parenless_calls::SUMMARY);
}

#[test]
#[ignore]
fn real_blog_model_class_methods_run_under_tsx() {
    // A model's class method calling another without a receiver answers
    // as CRuby does (#1236).
    let (emitted, errors) = model_class_methods::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { Comment } from \"./app/models/comment.js\";\nconsole.log([Comment.loud_tag(), Comment.tag_size()].join(\",\"));\n",
    );
    assert_eq!(out, model_class_methods::SUMMARY);
}

#[test]
#[ignore]
fn real_blog_nested_classes_run_under_tsx() {
    // Classes nested under a class and a module, raised, rescued and
    // built from another file, answer as CRuby does.
    let (emitted, errors) = nested_classes::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { Report } from \"./app/models/report.js\";\nconsole.log(Report.summary());\n",
    );
    assert_eq!(out, nested_classes::SUMMARY);
}

#[test]
#[ignore]
fn real_blog_class_constants_run_under_tsx() {
    // A class's own constants of each literal kind, read from a class
    // method, answer as CRuby does (#1223).
    let (emitted, errors) = class_constants::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { Scale } from \"./app/models/scale.js\";\nconsole.log(Scale.summary());\n",
    );
    assert_eq!(out, class_constants::SUMMARY);
}

#[test]
#[ignore]
fn real_blog_model_constant_reads_run_under_tsx() {
    // A model method reading its own constants through `fetch`, `key?`,
    // `keys`, `first` and `[]` answers as CRuby does (#1223 b).
    let (emitted, errors) = class_constants::gauge_overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { Gauge } from \"./app/models/gauge.js\";\nconst g = new Gauge();\ng.size = \"large\";\nconsole.log(g.summary() + \";\" + Gauge.heaviest() + \";\" + Gauge.probe());\n",
    );
    assert_eq!(out, format!("{};9;true,false,true", class_constants::GAUGE_SUMMARY));
}

#[test]
#[ignore]
fn real_blog_shadowing_constant_runs_under_tsx() {
    // A child's constant shadowing its parent's answers its own value, and
    // the parent's still answers the parent's (#1223).
    let (emitted, errors) = class_constants::shadow_overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { Dial } from \"./app/models/dial.js\";\nconsole.log(Dial.summary());\n",
    );
    assert_eq!(out, class_constants::SHADOW_SUMMARY);
}

#[test]
#[ignore]
fn real_blog_array_methods_run_under_tsx() {
    // The Array methods the emit translates answer as CRuby does,
    // empty-Array and not-found answers included.
    let (emitted, errors) = array_methods::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { ArrayProbe } from \"./app/models/array_probe.js\";\nconsole.log(JSON.stringify(ArrayProbe.run([1, 2, 3], [\"a\", \"b\"])));\n",
    );
    assert_eq!(out, array_methods::EXPECTED);
}

#[test]
#[ignore]
fn real_blog_number_methods_run_under_tsx() {
    // Integer and Float methods, the operators Ruby defines differently
    // (`/`, `%`, `<=>`, half-away rounding) and a chained `<<`.
    let (emitted, errors) = array_methods::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { NumberProbe } from \"./app/models/number_probe.js\";\nconsole.log(JSON.stringify(NumberProbe.run(7, 2, 2.5, [0])));\n",
    );
    assert_eq!(out, array_methods::NUMBER_EXPECTED);
}

#[test]
#[ignore]
fn real_blog_string_methods_run_under_tsx() {
    // Width padding, character-set specs (`a-z`, `^l`, intersections) and
    // the not-found answers Ruby gives.
    let (emitted, errors) = array_methods::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { StringProbe } from \"./app/models/string_probe.js\";\nconsole.log(JSON.stringify(StringProbe.run(\"hello\", \"  aa--bbb-\\n\", \"*\")));\n",
    );
    assert_eq!(out, array_methods::STRING_EXPECTED);
}

#[test]
#[ignore]
fn real_blog_ruby_truthiness_runs_under_tsx() {
    // `0` and `""` are true, `0 || 5` is 0, and a predicate block keeps them.
    let (emitted, errors) = truthiness::overlay().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        "import { TruthProbe } from \"./app/models/truth_probe.js\";\nconsole.log(JSON.stringify(TruthProbe.run(0, \"\", null, false, [], [\"\", \"a\"])));\n",
    );
    assert_eq!(out, truthiness::EXPECTED);
}

#[test]
#[ignore]
fn real_blog_i18n_locales_run_under_tsx() {
    // The answers `emit_and_run::i18n_locales` checks on CRuby, but for a String
    // locale named in an error: JS has no Symbol apart from String, so `"de"` shows as `:de`.
    let (emitted, errors) = i18n_locales_app::probe_app()
        .edit("app/models/article.rb", i18n_locales_app::IN_LOCALE, "")
        .emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(emitted.join("config/i18n_translations").exists());
    let out = run_under_tsx(
        &emitted,
        r#"import { Article } from "./app/models/article.js";
import { I18n } from "./src/i18n_locale.js";
const expect = (what: string, got: unknown, want: unknown) => {
  if (JSON.stringify(got) !== JSON.stringify(want)) throw new Error(what + ": " + JSON.stringify(got));
};
expect("lookups", Article.lookups(), ["Hello", "Hi <x>", "1 item", "2 items", "Translation missing: en.probe.nope", "Cześć", "fallback 3", "en"]);
expect("polish", Article.polish(), ["1 rzecz", "5 rzeczy", "English only", "Cześć", "Translation missing: pl.probe.nope", "Cześć", "Hi Ola", "pl"]);
expect("restored", I18n.locale(), "en");
expect("named", [Article.named("pl"), Article.named("en"), Article.named(null)], ["Cześć", "Hello", "Hello"]);
expect("dynamic", [Article.probe_key("greet"), Article.only_key("xx")], ["Hello", "Translation missing: en.probe.only_xx"]);
let message = "no raise";
try { Article.named("de"); } catch (e) { message = (e as Error).message; }
expect("named de", message, ":de is not a valid locale");
console.log("locales passed");
"#,
    );
    assert_eq!(out, "locales passed");
    std::fs::remove_file(emitted.join("check.ts")).unwrap();
    assert_tsc_passes("real-blog with two locales", &emitted);
}

#[test]
#[ignore]
fn real_blog_record_slice_runs_under_tsx() {
    // The answers `emit_and_run::record_slice` checks on CRuby.
    let (emitted, errors) = record_slice_app::probe_app().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let out = run_under_tsx(
        &emitted,
        r#"import { Article } from "./app/models/article.js";
const got = Article.sliced();
if (JSON.stringify(got) !== JSON.stringify(["2", "Sliced", "7", "1", "Other", "3", "Bound", "5", "4", "A comment body."])) throw new Error("sliced: " + JSON.stringify(got));
console.log("slice passed");
"#,
    );
    assert_eq!(out, "slice passed");
    std::fs::remove_file(emitted.join("check.ts")).unwrap();
    assert_tsc_passes("real-blog with record slices", &emitted);
}
