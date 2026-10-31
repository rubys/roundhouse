//! Go toolchain integration test — Phase 1 forcing function.
//!
//! Generates the emitted Go project for a fixture into a scratch dir
//! and runs `go vet ./app` against it. Go's `vet` parses + type-checks
//! the package without linking — closest equivalent to `cargo check` /
//! `crystal build --no-codegen` / `tsc --noEmit`.
//!
//! Scoped to the `./app` package, which as of Phase 4c contains
//! controllers, http runtime, and models all flat in one package.
//!
//! Marked `#[ignore]`; run with:
//!
//!     cargo test --test go_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/i18n_locales_app.rs"]
#[allow(dead_code)]
mod i18n_locales_app;
#[path = "support/record_slice_app.rs"]
#[allow(dead_code)]
mod record_slice_app;

use roundhouse::emit::go;
use roundhouse::ingest::ingest_app;

#[test]
fn real_blog_runtime_calls_use_qualified_class_names() {
    let mut app = ingest_app(roundhouse::fixtures::real_blog()).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = go::emit(&app);
    for (path, calls) in [
        ("app/v2/flash.go", vec!["NewActionDispatchFlashNow(self)", "NewActionDispatchFlash()"]),
        ("app/v2/router.go", vec!["return NewActionDispatchRouterMatchResult("]),
        ("app/v2/session.go", vec![
            "NewActionDispatchSession(self.ToH())",
            "NewActionDispatchSession()",
            "ActionDispatchSession_cookie_decode(fmt.Sprintf(\"%v\", parts[0]))",
            "ActionDispatchSession_cookie_decode(fmt.Sprintf(\"%v\", parts[1]))",
            "ActionDispatchSession_cookie_encode(k)",
            "ActionDispatchSession_cookie_encode(fmt.Sprintf(\"%v\", self.OpGet(k)))",
        ]),
    ] {
        let file = files.iter().find(|file| file.path == Path::new(path)).expect("runtime file");
        for call in calls {
            assert!(file.content.contains(call), "{path} must call {call}");
        }
    }
}

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-go-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = go::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

fn go_mod_tidy(scratch: &Path) {
    let tidy = Command::new("go")
        .arg("mod")
        .arg("tidy")
        .current_dir(scratch)
        .output()
        .expect("run go mod tidy");
    assert!(
        tidy.status.success(),
        "go mod tidy failed at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&tidy.stdout),
        String::from_utf8_lossy(&tidy.stderr),
    );
}

fn assert_go_passes(fixture: &str, scratch: &Path) {
    go_mod_tidy(scratch);
    let output = Command::new("go")
        .arg("vet")
        .arg("./app/v2/")
        .current_dir(scratch)
        .output()
        .expect("run go vet");

    assert!(
        output.status.success(),
        "go vet failed on emitted {fixture} project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_go_vet_passes() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-vet");
    generate_project(fixture, &scratch);
    assert_go_passes("real-blog", &scratch);
}

#[test]
#[ignore]
fn real_blog_go_test_passes() {
    // Phase 2 forcing function: emit real-blog and run `go test ./...`,
    // the same discovery the emitted README documents. Asserts zero
    // failures across the fixture, model, and controller tests.
    // `./app/v2/` alone would stay green if a later package grew tests.
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-test");
    generate_project(fixture, &scratch);
    go_mod_tidy(&scratch);

    let output = Command::new("go")
        .arg("test")
        .arg("./...")
        .current_dir(&scratch)
        .output()
        .expect("run go test");

    assert!(
        output.status.success(),
        "go test failed on emitted real-blog at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore]
fn real_blog_i18n_locales_run() {
    // The answers `emit_and_run::i18n_locales` checks on CRuby, but for a String
    // locale named in an error: Go has no Symbol apart from string, so `"de"` shows as `:de`.
    let (emitted, errors) = i18n_locales_app::probe_app()
        .edit("app/models/article.rb", i18n_locales_app::IN_LOCALE, "")
        .emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/i18n_probe_test.go"), r#"package v2

import (
	"reflect"
	"testing"
)

func TestI18nLocalesProbe(t *testing.T) {
	expect := func(what string, got, want interface{}) {
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("%s: %#v", what, got)
		}
	}
	expect("lookups", Article_lookups(), []string{"Hello", "Hi <x>", "1 item", "2 items", "Translation missing: en.probe.nope", "Cześć", "fallback 3", "en"})
	expect("polish", Article_polish(), []string{"1 rzecz", "5 rzeczy", "English only", "Cześć", "Translation missing: pl.probe.nope", "Cześć", "Hi Ola", "pl"})
	expect("restored", I18n_locale(), "en")
	expect("named", []string{Article_named("pl"), Article_named("en"), Article_named(nil)}, []string{"Cześć", "Hello", "Hello"})
	expect("dynamic", []string{Article_probe_key("greet"), Article_only_key("xx")}, []string{"Hello", "Translation missing: en.probe.only_xx"})
	func() {
		defer func() {
			e, ok := recover().(*I18nInvalidLocale)
			if !ok {
				t.Fatalf("named de: %#v", e)
			}
			expect("named de", e.Message, ":de is not a valid locale")
		}()
		Article_named("de")
	}()
}
"#).expect("write probe");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-run", "TestI18nLocalesProbe", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn real_blog_record_slice_runs() {
    // The answers `emit_and_run::record_slice` checks on CRuby.
    let (emitted, errors) = record_slice_app::probe_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/record_slice_probe_test.go"), r#"package v2

import (
	"reflect"
	"testing"
)

func TestRecordSliceProbe(t *testing.T) {
	if got := Article_sliced(); !reflect.DeepEqual(got, []string{"2", "Sliced", "7", "1", "Other", "3", "Bound", "5", "4", "A comment body."}) {
		t.Fatalf("sliced: %#v", got)
	}
}
"#).expect("write probe");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-run", "TestRecordSliceProbe", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore]
fn real_blog_flash_each_yields() {
    // A yielding runtime method takes the block as a Go function.
    let (emitted, errors) = emit_and_run::real_blog().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("app/v2/flash_each_test.go"), r#"package v2

import (
	"reflect"
	"testing"
)

func TestFlashEachYieldsPopulatedFields(t *testing.T) {
	flash := NewActionDispatchFlash(map[string]string{"alert": "Oops"})
	var got []string
	flash.Each(func(k string, v string) { got = append(got, k+"="+v) })
	if !reflect.DeepEqual(got, []string{"alert=Oops"}) {
		t.Fatalf("%#v", got)
	}
}
"#).expect("write test");
    go_mod_tidy(&emitted);
    let output = Command::new("go")
        .args(["test", "-count=1", "-run", "TestFlashEachYieldsPopulatedFields", "./app/v2/"])
        .current_dir(&*emitted)
        .output()
        .expect("run go test");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
