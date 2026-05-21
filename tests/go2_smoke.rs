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
use roundhouse::dialect::{
    AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param as DialectParam,
};
use roundhouse::effect::EffectSet;
use roundhouse::emit::{go, go2};
use roundhouse::expr::{Expr, ExprNode, LValue};
use roundhouse::ident::{ClassId, Symbol, VarId};
use roundhouse::ingest::ingest_app;
use roundhouse::span::Span;
use roundhouse::ty::{Param as TyParam, ParamKind, Ty};

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

/// Synthesize the module-singleton LibraryClass shape — `module
/// ActiveRecord; class << self; attr_accessor :adapter; end; end`
/// — and assert the emitted Go matches the module-slot architecture
/// contract: unit struct + per-slot package var + reader/writer
/// accessor functions, with `@adapter` reads/writes routing to the
/// namespaced slot (not `self.Adapter`).
///
/// Built from a synthesized `LibraryClass` rather than driven through
/// `GO_RUNTIME` because `active_record/base.rb` has many remaining
/// emit gaps (each-blocks, `.class` reflection, `Time` chain, etc.);
/// dropping it in whole would break `go vet` on the v2/ overlay. The
/// synthetic approach lets the module-singleton contract land
/// independently of the broader AR::Base widening.
#[test]
fn module_singleton_shape() {
    // `def self.adapter; @adapter; end` — synthesized from
    // `attr_accessor :adapter` inside `class << self`. Body is a
    // bare Ivar read; signature carries the slot's Ty so the
    // emitted `var ActiveRecord_adapter_slot <Ty>` declares the
    // right type. AdapterInterface stands in for what the RBS
    // gives in real ingest.
    let adapter_ty = Ty::Class {
        id: ClassId(Symbol::from("AdapterInterface")),
        args: vec![],
    };
    let reader = MethodDef {
        name: Symbol::from("adapter"),
        receiver: MethodReceiver::Class,
        params: vec![],
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Ivar { name: Symbol::from("adapter") },
        ),
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(adapter_ty.clone()),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("ActiveRecord")),
        kind: AccessorKind::AttributeReader,
        is_async: false,
        mutates_self: false,
    };
    // `def self.adapter=(value); @adapter = value; end`.
    let writer = MethodDef {
        name: Symbol::from("adapter="),
        receiver: MethodReceiver::Class,
        params: vec![DialectParam::positional(Symbol::from("value"))],
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: LValue::Ivar { name: Symbol::from("adapter") },
                value: Expr::new(
                    Span::synthetic(),
                    ExprNode::Var {
                        id: VarId(0),
                        name: Symbol::from("value"),
                    },
                ),
            },
        ),
        signature: Some(Ty::Fn {
            params: vec![TyParam {
                name: Symbol::from("value"),
                ty: adapter_ty.clone(),
                kind: ParamKind::Required,
            }],
            block: None,
            ret: Box::new(adapter_ty.clone()),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("ActiveRecord")),
        kind: AccessorKind::AttributeWriter,
        is_async: false,
        mutates_self: false,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("ActiveRecord")),
        is_module: true,
        parent: None,
        includes: vec![],
        methods: vec![reader, writer],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit module singleton");

    // Unit struct — the type name is preserved so `var x
    // *ActiveRecord` parses if anyone references it.
    assert!(
        emitted.contains("type ActiveRecord struct{}"),
        "missing unit-struct decl:\n{emitted}",
    );
    // Per-slot package var, namespaced by class name to avoid
    // cross-module collision.
    assert!(
        emitted.contains("var ActiveRecord_adapter_slot *AdapterInterface"),
        "missing slot var:\n{emitted}",
    );
    // Reader function — return slot, no receiver param.
    assert!(
        emitted.contains("func ActiveRecord_adapter() *AdapterInterface {"),
        "missing reader fn signature:\n{emitted}",
    );
    assert!(
        emitted.contains("return ActiveRecord_adapter_slot"),
        "reader body missing slot read:\n{emitted}",
    );
    // Writer function — sanitize maps `adapter=` to `adapter_eq`.
    // Writes target the slot; the value param is the single
    // positional arg.
    assert!(
        emitted.contains(
            "func ActiveRecord_adapter_eq(value *AdapterInterface)"
        ),
        "missing writer fn signature:\n{emitted}",
    );
    assert!(
        emitted.contains("ActiveRecord_adapter_slot = value"),
        "writer body missing slot assign:\n{emitted}",
    );
}

/// Pair to `module_singleton_shape` — assert that a plain class
/// (`is_module=false`) with the same attr_accessor still emits as a
/// struct field, NOT a module-singleton slot. Regression guard: the
/// module-singleton detection predicate must not fire on regular
/// classes; otherwise per-instance state would silently lift to
/// package vars.
#[test]
fn module_singleton_does_not_fire_on_plain_class() {
    let attr_ty = Ty::Class {
        id: ClassId(Symbol::from("AdapterInterface")),
        args: vec![],
    };
    let reader = MethodDef {
        name: Symbol::from("adapter"),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: Expr::new(
            Span::synthetic(),
            ExprNode::Ivar { name: Symbol::from("adapter") },
        ),
        signature: Some(Ty::Fn {
            params: vec![],
            block: None,
            ret: Box::new(attr_ty.clone()),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(Symbol::from("Configurable")),
        kind: AccessorKind::AttributeReader,
        is_async: false,
        mutates_self: false,
    };
    let class = LibraryClass {
        name: ClassId(Symbol::from("Configurable")),
        is_module: false,
        parent: None,
        includes: vec![],
        methods: vec![reader],
        origin: None,
    };

    let emitted = go2::emit_library_class(&class).expect("emit plain class");

    // Plain class → struct with a field; NOT a module-singleton
    // slot. Adapter shows up as a struct field rendered from the
    // attr_reader's signature.
    assert!(
        emitted.contains("type Configurable struct {")
            && emitted.contains("Adapter *AdapterInterface"),
        "plain class should emit struct field, not slot:\n{emitted}",
    );
    assert!(
        !emitted.contains("_slot"),
        "plain class accidentally hit module-singleton path:\n{emitted}",
    );
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
fn router_v2_shape() {
    let app = ingest_with_analyzer();
    let files = go2::emit_overlay_files(&app);
    let router = find_file(&files, "app/v2/router.go")
        .expect("v2/router.go missing from overlay output");
    let text = &router.content;

    // Class shape — attr_reader → struct fields, constructors.
    assert!(
        text.contains("type ActionDispatchRouterRoute struct {")
            && text.contains("Verb string")
            && text.contains("Pattern string"),
        "Route struct missing typed fields:\n{text}",
    );
    assert!(
        text.contains("func NewActionDispatchRouterRoute("),
        "Route constructor missing:\n{text}",
    );
    assert!(
        text.contains("PathParams map[string]string"),
        "MatchResult missing map[string]string field for path_params:\n{text}",
    );

    // Class methods — receive table as `[]*Route`, return `*MatchResult`.
    assert!(
        text.contains("table []*ActionDispatchRouterRoute"),
        "match() param missing typed slice:\n{text}",
    );
    assert!(
        text.contains(") *ActionDispatchRouterMatchResult {"),
        "match() return type not collapsed from nilable T:\n{text}",
    );

    // String method translations.
    assert!(
        text.contains("strings.ToUpper("),
        "method.to_s.upcase missing strings.ToUpper:\n{text}",
    );
    assert!(
        text.contains("strings.Split("),
        "split missing strings.Split:\n{text}",
    );
    assert!(
        text.contains("strings.HasPrefix("),
        "start_with? missing strings.HasPrefix:\n{text}",
    );

    // While loop + i++ + []= index assign.
    assert!(text.contains("for i < len("), "while loop missing for-emit:\n{text}");
    assert!(text.contains("i = i + 1"), "i += 1 missing reassign emit:\n{text}");
    assert!(
        text.contains("params[pp[1:]] = ap"),
        "[]= missing index-assign emit:\n{text}",
    );

    // `unless` → inverted if (no bare-nil then-branch).
    assert!(
        text.contains("if !(params == nil)"),
        "unless missing inverted-if emit:\n{text}",
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

    // Router smoke — pattern matching + table dispatch.
    let router_smoke = "package v2\n\
                        \n\
                        import \"testing\"\n\
                        \n\
                        func TestRouter_MatchPattern_Smoke(t *testing.T) {\n\
                        \tgot := ActionDispatchRouter_match_pattern(\"/articles/:id\", \"/articles/42\")\n\
                        \tif got == nil || got[\"id\"] != \"42\" {\n\
                        \t\tt.Fatalf(\"match_pattern result wrong: %#v\", got)\n\
                        \t}\n\
                        }\n\
                        \n\
                        func TestRouter_Match_Smoke(t *testing.T) {\n\
                        \ttable := []*ActionDispatchRouterRoute{\n\
                        \t\tNewActionDispatchRouterRoute(\"GET\", \"/articles\", \"articles\", \"index\"),\n\
                        \t\tNewActionDispatchRouterRoute(\"GET\", \"/articles/:id\", \"articles\", \"show\"),\n\
                        \t}\n\
                        \tres := ActionDispatchRouter_match(\"GET\", \"/articles/7\", table)\n\
                        \tif res == nil || res.Action != \"show\" || res.PathParams[\"id\"] != \"7\" {\n\
                        \t\tt.Fatalf(\"match result wrong: %#v\", res)\n\
                        \t}\n\
                        \tif ActionDispatchRouter_match(\"POST\", \"/articles/7\", table) != nil {\n\
                        \t\tt.Error(\"expected nil for unmatched method\")\n\
                        \t}\n\
                        }\n";
    std::fs::write(scratch.join("app/v2/router_smoke_test.go"), router_smoke)
        .expect("write router smoke");

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
