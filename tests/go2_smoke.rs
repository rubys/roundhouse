//! go2 overlay regression tests.
//!
//! Locks in the contract for what's landed in `src/emit/go2/`:
//!
//! - **Shape test** (unconditional): emit real-blog, assert the v2/
//!   inflector.go file is present and contains the expected function
//!   declaration. Catches accidental walker regressions, output-path
//!   reshuffles, or signature-decomposition breakage.
//!
//! - **Toolchain test** (`#[ignore]`): emit + `go vet ./app/v2` +
//!   `go test` against a smoke test that exercises
//!   `Inflector_pluralize`. Requires the Go toolchain on PATH;
//!   matches `tests/go_toolchain.rs`'s posture for legacy go.
//!
//! Run the toolchain test with:
//!
//!     cargo test --test go2_smoke -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::{go, go2};
use roundhouse::ingest::ingest_app;

const FIXTURE: &str = "fixtures/real-blog";

fn ingest_with_analyzer() -> roundhouse::App {
    let mut app = ingest_app(Path::new(FIXTURE)).expect("ingest real-blog");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn find_file<'a>(
    files: &'a [roundhouse::emit::EmittedFile],
    needle: &str,
) -> Option<&'a roundhouse::emit::EmittedFile> {
    files.iter().find(|f| f.path.to_string_lossy() == needle)
}

#[test]
fn json_builder_v2_shape() {
    let app = ingest_with_analyzer();
    let files = go2::emit_overlay_files(&app);
    let json_builder = find_file(&files, "app/v2/json_builder.go")
        .expect("v2/json_builder.go missing from overlay output");
    let text = &json_builder.content;

    // Module-level const initializers — Hash literal and regex —
    // emit as real values, not `var X interface{} = nil` placeholders.
    assert!(
        text.contains("var ESCAPES = map[string]string{"),
        "ESCAPES missing typed-map initializer:\n{text}",
    );
    assert!(
        text.contains("var ESCAPE_PATTERN = regexp.MustCompile("),
        "ESCAPE_PATTERN missing regexp.MustCompile initializer:\n{text}",
    );

    // Regex inside-class escape rewrite — `\b`/`\f` translate to
    // `\x08`/`\x0c` since Go's regexp rejects them inside `[]`.
    assert!(
        text.contains("\\\\x08") && text.contains("\\\\x0c"),
        "ESCAPE_PATTERN missing \\b/\\f → \\x08/\\x0c rewrite:\n{text}",
    );

    // gsub peephole — `s.gsub(REGEX, HASH)` → `REGEX.ReplaceAllStringFunc(s, func ...)`.
    assert!(
        text.contains("ESCAPE_PATTERN.ReplaceAllStringFunc(s, func(m string) string"),
        "encode_string missing gsub → ReplaceAllStringFunc translation:\n{text}",
    );

    // is_a? branches — singletons collapse to equality, mapped Tys
    // use type-assert if-init with branch-scoped ident substitution.
    assert!(text.contains("if v == true"), "TrueClass branch missing:\n{text}");
    assert!(
        text.contains("if i, ok := v.(int64); ok"),
        "Integer branch missing typed init:\n{text}",
    );
    assert!(
        text.contains("if s, ok := v.(string); ok"),
        "String branch missing typed init:\n{text}",
    );

    // Union{Nil,T} narrowing — `if s == nil` early return then
    // `s_str := s.(string)`.
    assert!(
        text.contains("s_str := s.(string)"),
        "encode_datetime missing nil-narrow assertion:\n{text}",
    );
}

#[test]
fn inflector_v2_shape() {
    let app = ingest_with_analyzer();
    let files = go2::emit_overlay_files(&app);
    let inflector = find_file(&files, "app/v2/inflector.go")
        .expect("v2/inflector.go missing from overlay output");

    let text = &inflector.content;
    // Package + import — `fmt.Sprintf` is referenced by the
    // Sprintf-emitted body so the file must `import "fmt"`.
    assert!(
        text.contains("package v2"),
        "v2/inflector.go missing `package v2` declaration:\n{text}",
    );
    assert!(
        text.contains("import \"fmt\""),
        "v2/inflector.go missing `import \"fmt\"`:\n{text}",
    );

    // Type declaration — Inflector is a Mode::Library entry so it
    // emits as an empty struct alongside its methods.
    assert!(
        text.contains("type Inflector struct{}"),
        "v2/inflector.go missing `type Inflector struct{{}}`:\n{text}",
    );

    // Function signature — class-method receiver flattens to a bare
    // `Inflector_pluralize`, with sig-derived `count int64, word string`
    // and return type `string`.
    assert!(
        text.contains("func Inflector_pluralize(count int64, word string) string"),
        "v2/inflector.go missing typed pluralize signature:\n{text}",
    );

    // Body — Ruby `count == 1 ? ... : ...` ternary lowered to Go
    // `if count == 1 { return ... } else { return ... }`. Both
    // branches return a `fmt.Sprintf(...)` call.
    assert!(
        text.contains("if count == 1 {"),
        "v2/inflector.go missing `if count == 1` branch:\n{text}",
    );
    assert!(
        text.contains("return fmt.Sprintf("),
        "v2/inflector.go body missing `return fmt.Sprintf(...)`:\n{text}",
    );
}

fn emit_to_scratch() -> PathBuf {
    let scratch = std::env::temp_dir().join("roundhouse-go2-smoke");
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(&scratch).expect("mkdir scratch");

    let app = ingest_with_analyzer();
    let mut files = go::emit(&app);
    files.extend(go2::emit_overlay_files(&app));

    for f in &files {
        let path = scratch.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::write(&path, &f.content).expect("write file");
    }
    scratch
}

#[test]
#[ignore]
fn inflector_v2_compiles_and_runs() {
    let scratch = emit_to_scratch();

    // Pin the behavioral contract — a Go test alongside the emitted
    // v2/inflector.go that exercises Inflector_pluralize.
    let smoke = "package v2\n\
                 \n\
                 import \"testing\"\n\
                 \n\
                 func TestInflectorPluralize_Smoke(t *testing.T) {\n\
                 \tcases := []struct{ count int64; word, want string }{\n\
                 \t\t{1, \"post\", \"1 post\"},\n\
                 \t\t{0, \"post\", \"0 posts\"},\n\
                 \t\t{5, \"post\", \"5 posts\"},\n\
                 \t\t{2, \"comment\", \"2 comments\"},\n\
                 \t}\n\
                 \tfor _, c := range cases {\n\
                 \t\tgot := Inflector_pluralize(c.count, c.word)\n\
                 \t\tif got != c.want {\n\
                 \t\t\tt.Errorf(\"Inflector_pluralize(%d,%q) = %q, want %q\", c.count, c.word, got, c.want)\n\
                 \t\t}\n\
                 \t}\n\
                 }\n";
    std::fs::write(scratch.join("app/v2/inflector_smoke_test.go"), smoke)
        .expect("write smoke test");

    // `go mod tidy` to populate go.sum from go.mod. Mirrors
    // tests/go_toolchain.rs.
    let tidy = Command::new("go")
        .arg("mod")
        .arg("tidy")
        .current_dir(&scratch)
        .output()
        .expect("run go mod tidy");
    assert!(
        tidy.status.success(),
        "go mod tidy failed:\n=== stderr ===\n{}",
        String::from_utf8_lossy(&tidy.stderr),
    );

    // `go vet ./app/v2` — parses + type-checks just the overlay
    // package. Scoped narrow so a legacy app/ regression doesn't
    // mask a v2 success or vice-versa.
    let vet = Command::new("go")
        .arg("vet")
        .arg("./app/v2")
        .current_dir(&scratch)
        .output()
        .expect("run go vet ./app/v2");
    assert!(
        vet.status.success(),
        "go vet ./app/v2 failed at {}:\n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&vet.stderr),
    );

    // JsonBuilder smoke — encode_string, encode_value, encode_datetime
    // behavior pinned against the emitted bodies.
    let json_smoke = "package v2\n\
                      \n\
                      import \"testing\"\n\
                      \n\
                      func TestJsonBuilder_EncodeValue_Smoke(t *testing.T) {\n\
                      \tcases := []struct{ in interface{}; want string }{\n\
                      \t\t{nil, \"null\"},\n\
                      \t\t{true, \"true\"},\n\
                      \t\t{false, \"false\"},\n\
                      \t\t{int64(42), \"42\"},\n\
                      \t\t{\"hi\", `\"hi\"`},\n\
                      \t}\n\
                      \tfor _, c := range cases {\n\
                      \t\tif got := JsonBuilder_encode_value(c.in); got != c.want {\n\
                      \t\t\tt.Errorf(\"encode_value(%v) = %q, want %q\", c.in, got, c.want)\n\
                      \t\t}\n\
                      \t}\n\
                      }\n\
                      \n\
                      func TestJsonBuilder_EncodeString_Smoke(t *testing.T) {\n\
                      \tif got := JsonBuilder_encode_string(`a\"b`); got != `a\\\"b` {\n\
                      \t\tt.Errorf(`encode_string(a\"b) = %q, want a\\\"b`, got)\n\
                      \t}\n\
                      \tif got := JsonBuilder_encode_string(\"a\\nb\"); got != `a\\nb` {\n\
                      \t\tt.Errorf(\"encode_string(a\\\\nb) = %q, want a\\\\nb\", got)\n\
                      \t}\n\
                      }\n";
    std::fs::write(
        scratch.join("app/v2/json_builder_smoke_test.go"),
        json_smoke,
    )
    .expect("write json_builder smoke");

    // `go test ./app/v2` — runs the smoke tests against the emitted
    // Inflector_pluralize and JsonBuilder_*.
    let test = Command::new("go")
        .arg("test")
        .arg("./app/v2")
        .current_dir(&scratch)
        .output()
        .expect("run go test ./app/v2");
    assert!(
        test.status.success(),
        "go test ./app/v2 failed at {}:\n=== stdout ===\n{}\n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&test.stdout),
        String::from_utf8_lossy(&test.stderr),
    );
}
