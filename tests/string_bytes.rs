//! A String's bytes are unsigned integers, including multi-byte UTF-8.
use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::Severity;
use roundhouse::emit;
use roundhouse::project::BuildTarget;
use roundhouse::ty::Ty;
use std::collections::HashMap;
use std::path::PathBuf;

/// Keep inference and the actual backend dispatch on the same ordinary Ruby
/// source; a block consumer must not be mistaken for array materialization.
fn analyzed() -> roundhouse::App {
    let source = r#"class ByteProbe
  def values
    "Aé東🎉".bytes
  end
  def nil_block
    "é".bytes(&nil)
  end
  def yielded
    "é".bytes { |byte| byte + 1 }
  end
end
"#;
    analyze_source(source)
}

/// Analyze a standalone support class without bringing Rails behavior into
/// byte-method typing and control-flow tests.
fn analyze_source(source: &str) -> roundhouse::App {
    let mut app = roundhouse::ingest::ingest_app_from_tree(HashMap::from([(
        PathBuf::from("app/lib/byte_probe.rb"),
        source.as_bytes().to_vec(),
    )]))
    .unwrap();
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// Extract the actual analyzed call, retaining a literal nil block operand so
/// primitive execution tests exercise the same classifier as project emission.
fn analyzed_call(name: &str) -> roundhouse::expr::Expr {
    let app = analyzed();
    let class = app
        .library_classes
        .iter()
        .find(|class| class.name.0.as_str() == "ByteProbe")
        .unwrap();
    let body = &class
        .methods
        .iter()
        .find(|method| method.name.as_str() == name)
        .unwrap()
        .body;
    if let roundhouse::expr::ExprNode::Seq { exprs } = &*body.node {
        exprs.last().unwrap().clone()
    } else {
        body.clone()
    }
}

/// Pin both the collection element type and the block yield/return contract;
/// an explicit nil block still requests array materialization.
#[test]
fn no_block_returns_integer_array_and_block_returns_string() {
    let app = analyzed();
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "ByteProbe")
        .unwrap();
    for (name, expected) in [
        (
            "values",
            Ty::Array {
                elem: std::sync::Arc::new(Ty::Int),
            },
        ),
        ("yielded", Ty::Str),
        (
            "nil_block",
            Ty::Array {
                elem: std::sync::Arc::new(Ty::Int),
            },
        ),
    ] {
        let method = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == name)
            .unwrap();
        assert_eq!(
            method.body.ty.as_ref(),
            Some(&expected),
            "{name}: {:?}",
            method.body
        );
    }
    let mut pending = vec![
        &class
            .methods
            .iter()
            .find(|m| m.name.as_str() == "yielded")
            .unwrap()
            .body,
    ];
    let mut yielded_reads = 0;
    while let Some(expr) = pending.pop() {
        if matches!(&*expr.node, roundhouse::expr::ExprNode::Var { name, .. } if name.as_str() == "byte")
        {
            assert_eq!(expr.ty, Some(Ty::Int));
            yielded_reads += 1;
        }
        expr.node.for_each_child(&mut |child| pending.push(child));
    }
    assert!(yielded_reads > 0, "block parameter was not inspected");
    let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

/// Give public diagnostics a normal model-method root; ordinary app/lib
/// bodies are deliberately outside the upstream diagnostic inventory.
fn analyzed_model(body: &str) -> roundhouse::App {
    let source =
        format!("class ByteProbe < ActiveRecord::Base\n  def values\n    {body}\n  end\nend\n");
    let mut app = roundhouse::ingest::ingest_app_from_tree(HashMap::from([
            (PathBuf::from("app/models/byte_probe.rb"), source.into_bytes()),
            (PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\n  create_table :byte_probes do |t|\n    t.string :name\n  end\nend\n".to_vec()),
        ])).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// Exercise the actual backend entry points, including the error ledger for
/// a supplied block rather than a plausible but side-effect-free array.
#[test]
fn every_bridge_dispatches_and_refuses_the_supplied_block() {
    let make_app = |body: &str| {
        let mut app = analyzed_model(body);
        roundhouse::session::analyze_and_lower(&mut app);
        app
    };
    let app = make_app("\"Aé東🎉\".bytes");
    let nil_app = make_app("\"Aé東🎉\".bytes(&nil)");
    let block_app = make_app("\"é\".bytes { |byte| byte + 1 }");
    let targets = [
        ("rust", BuildTarget::Rust, ".as_bytes().iter().map("),
        (
            "typescript",
            BuildTarget::Typescript,
            "new TextEncoder().encode(",
        ),
        (
            "crystal",
            BuildTarget::Crystal,
            ".bytes.map { |byte| byte.to_i64 }",
        ),
        ("python", BuildTarget::Python, ".encode(\"utf-8\")"),
        (
            "kotlin",
            BuildTarget::Kotlin,
            ".toByteArray(Charsets.UTF_8)",
        ),
        ("swift", BuildTarget::Swift, ".utf8.map { Int($0) }"),
        (
            "csharp",
            BuildTarget::CSharp,
            "System.Text.Encoding.UTF8.GetBytes(",
        ),
        ("go", BuildTarget::Go, "out[i] = int64(text[i])"),
        // Elixir has no native `While`. Leftover loops after
        // `while_to_recursion` hit the expression catch-all
        // (`elixir2`) rather than `library.rs` walkers. Whole-app
        // `target_files` therefore reports `While not supported` on
        // current `main` (runtime loops, not this bytes primitive).
        // The shared classifier still covers Elixir in
        // `shared_bridge_evaluates_receiver_once_and_rejects_arguments`.
    ];
    let library = analyzed();
    let class = library
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "ByteProbe")
        .unwrap();
    let output = |app: &roundhouse::App, target, name: &str| {
        // Python's model emitter does not carry arbitrary model methods;
        // its shared runtime/library expression entry point does.
        if target == BuildTarget::Python {
            let body = &class
                .methods
                .iter()
                .find(|m| m.name.as_str() == name)
                .unwrap()
                .body;
            vec![("probe.py".into(), emit::python::emit_expr_for_runtime(body))]
        } else {
            roundhouse::project::target_files(app, std::path::Path::new("."), target).unwrap()
        }
    };
    for (name, target, bridge) in targets {
        let (files, errors) = emit::diagnostics::scope(|| output(&app, target, "values"));
        assert!(
            files.iter().any(|(_, content)| content.contains(bridge)),
            "{name} did not dispatch String#bytes: {errors:?}"
        );
        // Exercise project admission even for Python, whose arbitrary model
        // methods use the separate library expression path below.
        let (result, diagnostics) = emit::diagnostics::scope(|| {
            roundhouse::project::target_files(&nil_app, std::path::Path::new("."), target)
        });
        assert!(result.is_ok(), "{name} refused literal &nil: {result:?}");
        assert!(diagnostics.is_empty(), "{name}: {diagnostics:?}");
        let (files, diagnostics) =
            emit::diagnostics::scope(|| output(&nil_app, target, "nil_block"));
        assert!(diagnostics.is_empty(), "{name}: {diagnostics:?}");
        assert!(
            files.iter().any(|(_, content)| content.contains(bridge)),
            "{name} did not materialize literal &nil"
        );
        let (_, diagnostics) = emit::diagnostics::scope(|| output(&block_app, target, "yielded"));
        assert!(
            diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error && d.message.contains("String#bytes")),
            "{name} discarded the block: {diagnostics:?}"
        );
    }
}

/// Ruby and Spinel share emission, so canonicalize only the typed no-block byte
/// call; unrelated forwarding and effectful block operands must survive.
#[test]
fn ruby_family_canonicalizes_only_literal_nil_bytes() {
    let app = analyze_source(
        r#"class ByteProbe
  def values
    "é".bytes(&nil)
  end
  def unrelated
    "é".upcase(&nil)
  end
  def effectful
    "é".bytes(&(puts("side effect"); nil))
  end
  def yielded
    "é".bytes { |byte| byte + 1 }
  end
end
"#,
    );
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "ByteProbe")
        .unwrap();
    let output = |name: &str| {
        emit::ruby::emit_expr(
            &class
                .methods
                .iter()
                .find(|m| m.name.as_str() == name)
                .unwrap()
                .body,
        )
    };
    let values = output("values");
    assert!(values.contains(".bytes"), "{values}");
    assert!(!values.contains('&'), "{values}");
    let unrelated = output("unrelated");
    assert!(unrelated.contains("&nil"), "{unrelated}");
    let effectful = output("effectful");
    assert!(
        effectful.contains('&') && effectful.contains("side effect"),
        "{effectful}"
    );
    let yielded = output("yielded");
    assert!(
        yielded.contains("|byte|") && yielded.contains("byte + 1"),
        "{yielded}"
    );
}

/// Lowering may represent an unsupported expression as a diagnostic-bearing
/// nil. It must remain a refusal or raise, never become an omitted block.
#[test]
fn diagnostic_nil_block_is_not_discarded() {
    use emit::shared::string_bytes::{self, Target};
    use roundhouse::diagnostic::DiagnosticKind;
    use roundhouse::expr::ExprNode;
    let mut call = analyzed_call("nil_block");
    let ExprNode::Send {
        block: Some(block), ..
    } = &mut *call.node
    else {
        panic!("expected the literal-nil byte call");
    };
    block.diagnostic = Some(DiagnosticKind::Unsupported {
        target: None,
        construct: "forwarded operand".into(),
        detail: "preserve this refusal".into(),
    });
    assert!(!string_bytes::materializes_array(&call));
    let ruby = emit::ruby::emit_expr(&call);
    assert!(
        ruby.contains("raise") && ruby.contains("forwarded operand not supported"),
        "{ruby}"
    );
    let (_, diagnostics) = emit::diagnostics::scope(|| {
        string_bytes::emit(&call, Target::Python, |_| panic!("must not materialize"))
    });
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("String#bytes")),
        "{diagnostics:?}"
    );
}

/// The bytes exemption is local to the implemented primitive. It must not
/// admit unrelated forwarding or skip validation within an effectful receiver.
#[test]
fn nil_bytes_exemption_preserves_other_forwarding_boundaries() {
    for source in [
        "class ByteProbe < ActiveRecord::Base\n  def values\n    \"A\".upcase(&nil)\n  end\nend\n",
        "class ByteProbe < ActiveRecord::Base\n  def receiver\n    \"A\"\n  end\n  def values\n    receiver(&nil).bytes(&nil)\n  end\nend\n",
        "class ByteProbe < ActiveRecord::Base\n  def values\n    \"A\".bytes(&(puts(\"side effect\"); nil))\n  end\nend\n",
    ] {
        let mut app = roundhouse::ingest::ingest_app_from_tree(HashMap::from([
            (PathBuf::from("app/models/byte_probe.rb"), source.as_bytes().to_vec()),
            (PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\n  create_table :byte_probes do |t|\n    t.string :name\n  end\nend\n".to_vec()),
        ])).unwrap();
        roundhouse::session::analyze_and_lower(&mut app);
        let mut pending: Vec<_> = app
            .models
            .iter()
            .flat_map(|model| model.methods().map(|method| &method.body))
            .collect();
        let mut materializing_call = false;
        while let Some(expression) = pending.pop() {
            materializing_call |= emit::shared::string_bytes::materializes_array(expression);
            expression
                .node
                .for_each_child(&mut |child| pending.push(child));
        }
        assert_eq!(
            materializing_call,
            source.contains("receiver(&nil).bytes"),
            "the nested receiver control must reach the bytes exemption: {source}"
        );
        let (result, diagnostics) = emit::diagnostics::scope(|| {
            roundhouse::project::target_files(&app, std::path::Path::new("."), BuildTarget::Rust)
        });
        assert!(
            result.is_err(),
            "unrelated forwarding was accepted: {source}"
        );
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| matches!(&diagnostic.kind,
            roundhouse::diagnostic::DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == "forwarded_proc")),
            "{source}: {diagnostics:?}"
        );
    }
}

/// The bridge must not duplicate an effectful operand, and malformed calls
/// must fail before being lowered into a supported array operation.
#[test]
fn shared_bridge_evaluates_receiver_once_and_rejects_arguments() {
    use emit::shared::string_bytes::{self, Target};
    use roundhouse::expr::ExprNode;
    let app = analyzed();
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "ByteProbe")
        .unwrap();
    let mut expression = class
        .methods
        .iter()
        .find(|m| m.name.as_str() == "values")
        .unwrap()
        .body
        .clone();
    // Method bodies may contain their single expression in a Seq.
    if let ExprNode::Seq { exprs } = &*expression.node {
        expression = exprs.last().unwrap().clone();
    }
    for target in [
        Target::Rust,
        Target::TypeScript,
        Target::Crystal,
        Target::Python,
        Target::Kotlin,
        Target::Swift,
        Target::CSharp,
        Target::Go,
        Target::Elixir,
    ] {
        for method in ["values", "nil_block"] {
            let call = analyzed_call(method);
            assert!(string_bytes::materializes_array(&call));
            let mut calls = 0;
            let output = string_bytes::emit(&call, target, |_| {
                calls += 1;
                "effectful_receiver()".into()
            })
            .unwrap();
            assert_eq!(calls, 1);
            assert_eq!(
                output.matches("effectful_receiver()").count(),
                1,
                "{output}"
            );
        }
    }
    let argument = expression.clone();
    if let ExprNode::Send { args, .. } = &mut *expression.node {
        args.push(argument);
    }
    let (_, diagnostics) = emit::diagnostics::scope(|| {
        string_bytes::emit(&expression, Target::Python, |_| {
            panic!("invalid call must not materialize bytes")
        })
    });
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("String#bytes")),
        "{diagnostics:?}"
    );
}

/// Execute generated primitive expressions in the installed SDKs. Keeping this
/// separate from shape checks makes unavailable SDK coverage explicit.
#[test]
#[ignore = "requires rustc and python3"]
fn generated_byte_expressions_execute_with_unsigned_values_and_one_receiver_call() {
    use emit::shared::string_bytes::{self, Target};
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("roundhouse-string-bytes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rust = string_bytes::render(Target::Rust, "effectful_receiver()");
    let nil_rust = string_bytes::emit(&analyzed_call("nil_block"), Target::Rust, |_| {
        "effectful_receiver()".into()
    })
    .unwrap();
    let source = format!(
        r#"
use std::sync::atomic::{{AtomicUsize, Ordering}};
static CALLS: AtomicUsize = AtomicUsize::new(0);
/// Count source receiver evaluations for the emitted primitive expressions.
fn effectful_receiver() -> String {{ CALLS.fetch_add(1, Ordering::SeqCst); "A\0é東🎉".to_string() }}
/// Execute both omitted-block and literal-nil forms with identical byte checks.
fn main() {{
  let bytes: Vec<i64> = {rust};
  assert_eq!(bytes, vec![65, 0, 195, 169, 230, 157, 177, 240, 159, 142, 137]);
  assert_eq!(CALLS.load(Ordering::SeqCst), 1);
  let nil_bytes: Vec<i64> = {nil_rust};
  assert_eq!(nil_bytes, bytes);
  assert_eq!(CALLS.load(Ordering::SeqCst), 2);
  let empty: Vec<i64> = {empty};
  assert!(empty.is_empty());
}}
"#,
        empty = string_bytes::render(Target::Rust, "\"\"")
    );
    std::fs::write(dir.join("probe.rs"), source).unwrap();
    let compiled = Command::new("rustc")
        .arg(dir.join("probe.rs"))
        .arg("-o")
        .arg(dir.join("probe"))
        .output()
        .expect("rustc");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    assert!(Command::new(dir.join("probe")).status().unwrap().success());
    let python = string_bytes::render(Target::Python, "effectful_receiver()");
    let nil_python = string_bytes::emit(&analyzed_call("nil_block"), Target::Python, |_| {
        "effectful_receiver()".into()
    })
    .unwrap();
    let source = format!(
        r#"
calls = 0
def effectful_receiver():
    """Count source receiver evaluations for the emitted primitive expressions."""
    global calls
    calls += 1
    return "A\0é東🎉"
assert {python} == [65, 0, 195, 169, 230, 157, 177, 240, 159, 142, 137]
assert calls == 1
assert {nil_python} == [65, 0, 195, 169, 230, 157, 177, 240, 159, 142, 137]
assert calls == 2
assert {empty} == []
"#,
        empty = string_bytes::render(Target::Python, "\"\"")
    );
    std::fs::write(dir.join("probe.py"), source).unwrap();
    let output = Command::new("python3")
        .arg(dir.join("probe.py"))
        .output()
        .expect("python3");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A break escaping the bytes iterator changes its result independently of
/// the receiver type; it must not receive a clean but false String signature.
#[test]
fn escaping_bytes_block_break_has_an_explicit_diagnostic() {
    for body in ["break 7", "break"] {
        let app = analyzed_model(&format!("\"x\".bytes {{ {body} }}"));
        let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors
                .iter()
                .any(|d| d.message.contains("String#bytes") && d.message.contains("break")),
            "{body}: {errors:?}"
        );
    }
}

/// A nested loop or iterator consumes its own break; refusing those would
/// reject an ordinary bytes block that still returns its String receiver.
#[test]
fn nested_loop_and_iterator_breaks_do_not_escape_bytes() {
    for body in [
        "while byte > 0; break; end",
        "[byte].each { |value| break value }",
    ] {
        let app = analyzed_model(&format!("\"x\".bytes {{ |byte| {body} }}"));
        let errors: Vec<_> = roundhouse::analyze::diagnose(&app)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{body}: {errors:?}");
        assert_eq!(
            app.models[0].methods().next().unwrap().body.ty,
            Some(Ty::Str)
        );
    }
}
