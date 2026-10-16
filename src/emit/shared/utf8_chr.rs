//! Unicode scalar construction for validated shared-runtime UTF-8.
//!
//! Classification is shared; only scalar construction/error syntax varies.
//! The receiver is evaluated once, including on an invalid code point.
//! Shared callers validate first. This narrow bridge does not establish
//! application-level Encoding support or general Ruby encoding-error parity.
use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

/// Target syntax varies; validation/route decoding remains shared Ruby.
#[derive(Clone, Copy)]
pub enum Target {
    Rust,
    TypeScript,
    Crystal,
    Python,
    Kotlin,
    Swift,
    CSharp,
    Go,
    Elixir,
}

impl Target {
    /// Attribute unsupported call shapes to their actual emission lane.
    fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Crystal => "crystal",
            Self::Python => "python",
            Self::Kotlin => "kotlin",
            Self::Swift => "swift",
            Self::CSharp => "csharp",
            Self::Go => "go",
            Self::Elixir => "elixir",
        }
    }
}

/// Recognize the complete internal UTF-8 constructor before any backend drops
/// operands. Supplied blocks and diagnostic-bearing operands remain explicit
/// refusals; only the no-block scalar constructor needed by shared Ruby emits.
pub fn emit(
    e: &Expr,
    target: Target,
    emit_receiver: impl FnOnce(&Expr) -> String,
) -> Option<String> {
    if e.diagnostic.is_some() {
        return None;
    }
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        block,
        ..
    } = &*e.node
    else {
        return None;
    };
    if method.as_str() != "chr"
        || recv.ty.as_ref() != Some(&Ty::Int)
        || args.len() != 1
        || !matches!(&*args[0].node, ExprNode::Const { path }
            if path.iter().map(|s| s.as_str()).eq(["Encoding", "UTF_8"]))
    {
        return None;
    }
    let unsupported = if block.is_some() {
        Some(
            "the shared UTF-8 constructor does not implement supplied blocks or forwarded operands",
        )
    } else if recv.diagnostic.is_some() || args[0].diagnostic.is_some() {
        Some("the shared UTF-8 constructor cannot erase an operand diagnostic")
    } else {
        None
    };
    Some(if let Some(detail) = unsupported {
        crate::emit::diagnostics::report_unsupported(
            e.span,
            target.name(),
            "Integer#chr(Encoding::UTF_8)",
            detail,
        )
    } else {
        render(target, &emit_receiver(recv))
    })
}

/// Render one scalar construction, evaluating its receiver exactly once.
pub fn render(target: Target, recv: &str) -> String {
    match target {
        Target::Rust => format!("u32::try_from({recv}).ok().and_then(char::from_u32).expect(\"invalid Unicode codepoint\").to_string()"),
        Target::TypeScript => format!("((cp: number): string => {{ if (cp < 0 || cp > 1114111 || (cp >= 55296 && cp <= 57343)) throw new RangeError(\"invalid Unicode codepoint\"); return String.fromCodePoint(cp); }})({recv})"),
        Target::Crystal => format!("({recv}).chr.to_s"),
        Target::Python => format!("(lambda cp: chr(cp) if not 55296 <= cp <= 57343 else (_ for _ in ()).throw(ValueError(\"invalid Unicode codepoint\")))({recv})"),
        Target::Kotlin => format!("({recv}).let {{ cp -> require(cp >= 0 && cp <= 1114111 && (cp < 55296 || cp > 57343)) {{ \"invalid Unicode codepoint\" }}; String(Character.toChars(cp.toInt())) }}"),
        Target::Swift => format!("String(UnicodeScalar({recv})!)"),
        Target::CSharp => format!("char.ConvertFromUtf32(checked((int)({recv})))"),
        Target::Go => format!("func(cp int64) string {{ if cp < 0 || cp > 1114111 || (cp >= 55296 && cp <= 57343) {{ panic(\"invalid Unicode codepoint\") }}; return string(rune(cp)) }}({recv})"),
        Target::Elixir => format!("<<({recv})::utf8>>"),
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime_src::parse_methods_with_rbs;

    /// Ingest the exact shared-runtime call shape without application Encoding
    /// admission, which deliberately remains unsupported by the frontend.
    fn character_call(suffix: &str) -> crate::expr::Expr {
        let source = format!(
            "module Probe\n def self.character\n  233.chr(Encoding::UTF_8{suffix})\n end\nend\n"
        );
        let methods = parse_methods_with_rbs(
            &source,
            "module Probe\n def self.character: () -> String\nend\n",
        )
        .unwrap();
        let mut call = methods[0].body.clone();
        if let crate::expr::ExprNode::Seq { exprs } = &*call.node {
            call = exprs.last().unwrap().clone();
        }
        call
    }

    /// Every bridge must evaluate one receiver and retain a refusal for
    /// forwarded operands or diagnostic-bearing encoding arguments.
    #[test]
    fn utf8_constructor_guards_whole_calls_on_every_target() {
        use super::{emit, Target};
        use crate::expr::ExprNode;
        let plain = character_call("");
        let forwarded = character_call(", &(puts \"BLOCK_OPERAND_EVALUATED\"; nil)");
        let nil_block = character_call(", &nil");
        let mut diagnostic = plain.clone();
        let ExprNode::Send { args, .. } = &mut *diagnostic.node else {
            panic!("constructor call");
        };
        args[0].diagnostic = Some(crate::diagnostic::DiagnosticKind::Unsupported {
            target: None,
            construct: crate::ident::Symbol::new("encoding_operand"),
            detail: "encoding operand remains unsupported".into(),
        });
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
            let mut calls = 0;
            let output = emit(&plain, target, |_| {
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
            for call in [&forwarded, &nil_block, &diagnostic] {
                let (output, diagnostics) = crate::emit::diagnostics::scope(|| {
                    emit(call, target, |_| {
                        panic!("refused call must not materialize a scalar")
                    })
                });
                assert!(output.is_some());
                assert!(
                    diagnostics
                        .iter()
                        .any(|d| d.severity == crate::diagnostic::Severity::Error
                            && d.message.contains("Integer#chr")),
                    "{}: {diagnostics:?}",
                    target.name()
                );
            }
        }
    }

    /// Run scalar construction and defensive Rust guards; error classes are not
    /// a claim of general Ruby Encoding conformance.
    #[test]
    fn utf8_character_emitted_rust_executes() {
        let methods = parse_methods_with_rbs(
            "module Probe\n  def self.character(cp)\n    cp.chr(Encoding::UTF_8)\n  end\nend\n",
            "module Probe\n  def self.character: (Integer cp) -> String\nend\n",
        )
        .expect("type UTF-8 character source");
        let source =
            crate::emit::rust::expr::with_emit_ctx(crate::emit::rust::EmitCtx::default(), || {
                crate::emit::rust::library::emit_module(&methods).unwrap()
            });
        let path = std::env::temp_dir().join(format!("roundhouse-utf8-chr-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let effectful = super::emit(&character_call(""), super::Target::Rust, |_| {
            "effectful_receiver()".into()
        })
        .unwrap();
        let main = format!(
            r#"{source}
use std::sync::atomic::{{AtomicUsize, Ordering}};
static CALLS: AtomicUsize = AtomicUsize::new(0);
/// Count receiver evaluation in the actual emitted primitive expression.
fn effectful_receiver() -> i64 {{ CALLS.fetch_add(1, Ordering::SeqCst); 233 }}
/// Exercise Unicode scalar boundaries through the actual emitted constructor.
fn main() {{
    assert_eq!({effectful}, "é");
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(character(0), "\0");
    assert_eq!(character(43), "+");
    assert_eq!(character(233), "é");
    assert_eq!(character(26481), "東");
    assert_eq!(character(127881), "🎉");
    assert_eq!(character(1114111), "\u{{10ffff}}");
    for cp in [-1, 55296, 57343, 1114112, 4294967296] {{
        assert!(std::panic::catch_unwind(|| character(cp)).is_err());
    }}
}}
"#
        );
        std::fs::write(path.join("main.rs"), &main).unwrap();
        let build = std::process::Command::new("rustc")
            .args(["--edition=2021", "main.rs", "-o", "probe"])
            .current_dir(&path)
            .output()
            .expect("execute rustc");
        assert!(
            build.status.success(),
            "{main}\n{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = std::process::Command::new(path.join("probe"))
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    /// Compile the actual typed shared decoder methods, not a Rust reimplementation.
    #[test]
    fn percent_capture_helpers_emitted_rust_execute() {
        let ruby = std::fs::read_to_string("runtime/ruby/action_dispatch/router.rb").unwrap();
        let rbs = std::fs::read_to_string("runtime/ruby/action_dispatch/router.rbs").unwrap();
        let names = [
            "decode_captures",
            "capture_pairs",
            "capture_part",
            "decode_capture",
            "utf8_width",
            "utf8_character",
            "percent_bytes",
            "percent_advance",
            "percent_byte",
            "capture_byte",
            "hex_digit",
        ];
        // Module-flat parsing needs the same cross-method return registry
        // as the runtime typing sweep. An empty registry leaves helper calls
        // unresolved and cannot establish emitted-runtime correctness.
        let mut registry = std::collections::HashMap::new();
        for (owner, signatures) in crate::rbs::parse_app_signatures(&rbs).unwrap() {
            let short = crate::ident::ClassId(crate::ident::Symbol::from(
                owner.0.as_str().rsplit("::").next().unwrap(),
            ));
            for (name, signature) in signatures {
                let crate::ty::Ty::Fn { ret, .. } = signature else {
                    panic!("method signature")
                };
                for id in [owner.clone(), short.clone()] {
                    registry
                        .entry(id)
                        .or_insert_with(crate::analyze::ClassInfo::default)
                        .instance_methods
                        .insert(name.clone(), std::sync::Arc::unwrap_or_clone(ret.clone()));
                }
            }
        }
        let methods: Vec<_> =
            crate::runtime_src::parse_methods_with_rbs_in_ctx(&ruby, &rbs, &registry)
                .unwrap()
                .into_iter()
                .filter(|m| names.contains(&m.name.as_str()))
                .collect();
        assert_eq!(methods.len(), names.len());
        let emitted =
            crate::emit::rust::expr::with_emit_ctx(crate::emit::rust::EmitCtx::default(), || {
                crate::emit::rust::library::emit_module(&methods).unwrap()
            });
        let path =
            std::env::temp_dir().join(format!("roundhouse-route-capture-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        let errors = std::fs::canonicalize("runtime/rust/errors_ext.rs").unwrap();
        let main = format!("use std::collections::HashMap;\n#[path = {errors:?}] mod errors_ext;\nuse errors_ext::{{raise, ArgumentError}};\n{emitted}\n{}", r#"
/// Check decoded text, byte combination, and rejection without claiming raw-invalid String transport.
fn main() {
    for (input, expected) in [
        ("+%2B%20%252F", "++ %2F"),
        ("café%20%E6%9D%B1%E4%BA%AC%F0%9F%8E%89", "café 東京🎉"),
        ("dir%2finside/file%20name", "dir/inside/file name"),
        ("1%2Ejson", "1.json"), ("%GG%2%%2G", "%GG%2%%2G"), ("%00", "\0"), ("%2500", "%00")
    ] {
        assert_eq!(decode_capture(input), expected, "{input}");
    }
    // Rust String cannot carry the raw-invalid intermediate Rack bytes.
    // Feed those bytes to the actual emitted byte/scalar helpers instead;
    // this checks combination semantics without claiming String transport.
    for (raw, expected_bytes, width, expected) in [
        (vec![37, 67, 51, 169], vec![195, 169], 2, "é"),
        (vec![195, 37, 65, 57], vec![195, 169], 2, "é"),
        (vec![37, 70, 48, 37, 57, 70, 142, 37, 56, 57], vec![240, 159, 142, 137], 4, "🎉"),
    ] {
        let bytes = percent_bytes(raw);
        assert_eq!(bytes, expected_bytes);
        let first = bytes[0];
        assert_eq!(utf8_character(bytes, 0, first, width), expected);
    }
    let params = HashMap::from([("id".to_string(), "%2B1".to_string())]);
    assert_eq!(decode_captures(params)["id"], "+1");
    for input in ["%FF", "%C0%AF", "%E0%80%AF", "%C2", "%ED%A0%80", "%F4%90%80%80", "%E2%82", "%C2%C0", "%C2%FF"] {
        assert!(std::panic::catch_unwind(|| decode_capture(input)).is_err());
    }
}
"#);
        std::fs::write(path.join("main.rs"), &main).unwrap();
        let built = std::process::Command::new("rustc")
            .args(["--edition=2021", "main.rs", "-o", "probe"])
            .current_dir(&path)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{main}\n{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let run = std::process::Command::new(path.join("probe"))
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    /// Actual shared-runtime dispatch must refuse evaluated &operands with an
    /// error; app-level Encoding admission remains a separate boundary.
    #[test]
    fn utf8_character_does_not_drop_effectful_forwarded_operands() {
        let methods = crate::runtime_src::parse_methods_with_rbs(
            "module Probe\n  def self.character\n    233.chr(Encoding::UTF_8, &(puts \"BLOCK_OPERAND_EVALUATED\"; nil))\n  end\nend\n",
            "module Probe\n  def self.character: () -> String\nend\n",
        ).expect("type shared-runtime call");
        for (target, emit) in [
            (
                "typescript",
                crate::emit::typescript::emit_expr_for_runtime as fn(&crate::expr::Expr) -> String,
            ),
            (
                "csharp",
                crate::emit::csharp::emit_expr_for_runtime as fn(&crate::expr::Expr) -> String,
            ),
        ] {
            let (text, diagnostics) = crate::emit::diagnostics::scope(|| emit(&methods[0].body));
            assert!(
                diagnostics.iter().any(|d| d.severity == crate::diagnostic::Severity::Error
                    && d.message.contains("Integer#chr")),
                "{target} did not refuse the evaluated &operand: {text}; {diagnostics:?}"
            );
        }
    }
}
