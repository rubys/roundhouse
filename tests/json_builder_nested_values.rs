//! `JsonBuilder.encode_value` of an Array or a Hash on the strict
//! targets whose untyped value is a dynamic one: Rust's
//! `serde_json::Value` and Go's `interface{}`. The runtime narrows with
//! `is_a?(Array)` / `is_a?(Hash)`, and each target has to read the
//! narrowed value as its array or object. The CRuby side is
//! `jbuilder_array_value.rs`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

// On a model: C# emits no plain Ruby class.
const NARROWED_WRITE: &str = r#"class Article < ApplicationRecord
  def self.append_three(v)
    if v.is_a?(Array)
      v << 3
    end
    nil
  end

  def self.put_k(v)
    if v.is_a?(Hash)
      v["k"] = 1
    end
    nil
  end
"#;

const EXPECTED: &str = r#"[["b",null],[2,1.5],[],"x",{"name":"b"}]"#;

fn run(dir: &std::path::Path, program: &str, args: &[&str]) {
    let output = std::process::Command::new(program)
        .args(args)
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("run {program}: {e}"));
    assert!(
        output.status.success(),
        "{program} {args:?} in {}\n{}\n{}",
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires the Rust toolchain; builds an emitted app"]
fn nested_values_encode_on_rust() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::create_dir_all(emitted.join("tests")).unwrap();
    std::fs::write(
        emitted.join("tests/json_builder_nested_values.rs"),
        format!(
            "#[test]\nfn nested_values() {{\n    let v = serde_json::json!([[\"b\", null], [2, 1.5], [], \"x\", {{\"name\": \"b\"}}]);\n    assert_eq!(app::json_builder::JsonBuilder::encode_value(v), {EXPECTED:?});\n}}\n"
        ),
    )
    .unwrap();
    run(&emitted, "cargo", &["test", "--quiet", "--test", "json_builder_nested_values"]);
}

#[test]
#[ignore = "requires Go; builds an emitted app"]
fn nested_values_encode_on_go() {
    let (emitted, errors) = emit_and_run::real_blog().emit(BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(
        emitted.join("app/v2/json_builder_nested_values_test.go"),
        format!(
            "package v2\n\nimport \"testing\"\n\nfunc TestEncodeNestedValues(t *testing.T) {{\n\tv := []any{{[]any{{\"b\", nil}}, []any{{int64(2), 1.5}}, []any{{}}, \"x\", map[string]any{{\"name\": \"b\"}}}}\n\tif got := JsonBuilder_encode_value(v); got != {EXPECTED:?} {{\n\t\tt.Fatalf(\"got %s\", got)\n\t}}\n}}\n"
        ),
    )
    .unwrap();
    run(&emitted, "go", &["mod", "tidy"]);
    run(&emitted, "go", &["test", "./app/v2/", "-run", "TestEncodeNestedValues"]);
}

#[test]
#[ignore = "requires .NET; builds an emitted app"]
fn nested_values_encode_on_csharp() {
    let (emitted, errors) = emit_and_run::real_blog().edit("app/models/article.rb", "class Article < ApplicationRecord\n", NARROWED_WRITE).emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    // A `List<long>` too: it is no `IEnumerable<object>`, so a generic pattern would miss it.
    std::fs::write(
        emitted.join("tests/JsonBuilderNestedValuesTest.cs"),
        format!(
            "using System.Collections.Generic;\nusing Xunit;\n\nnamespace Roundhouse;\n\npublic class JsonBuilderNestedValuesTest\n{{\n    [Fact]\n    public void NestedValues() {{\n        var v = new List<object?> {{ new List<object?> {{ \"b\", null }}, new List<object?> {{ 2L, 1.5 }}, new List<object?>(), \"x\", new Dictionary<string, object?> {{ [\"name\"] = \"b\" }} }};\n        Assert.Equal({EXPECTED:?}, JsonBuilder.EncodeValue(v));\n        Assert.Equal(\"[1,2]\", JsonBuilder.EncodeValue(new List<long> {{ 1L, 2L }}));\n    }}\n\n    [Fact]\n    public void NarrowedWritesReachTheOriginal() {{\n        var l = new List<long> {{ 1L }};\n        Article.AppendThree(l);\n        Assert.Equal(new List<long> {{ 1L, 3L }}, l);\n        var h = new Dictionary<string, object?>();\n        Article.PutK(h);\n        Assert.Equal(1L, h[\"k\"]);\n    }}\n}}\n"
        ),
    )
    .unwrap();
    run(&emitted, "dotnet", &["test", "tests/App.Tests.csproj", "--filter", "FullyQualifiedName~JsonBuilderNestedValuesTest"]);
}
