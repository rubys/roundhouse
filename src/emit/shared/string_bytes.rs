//! String#bytes materializes the receiver once as unsigned byte integers.
use crate::expr::{Expr, ExprNode, Literal};
use crate::ty::Ty;

/// Targets whose string representation needs a byte-array bridge.
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
    /// Use the diagnostic ledger's target name when refusing a call shape.
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

enum Call<'a> {
    Array(&'a Expr),
    Unsupported(&'static str),
}

/// Keep call recognition and support policy shared by project validation and
/// backend dispatch. Literal `&nil` passes no block in Ruby; arbitrary block
/// operands still need an implementation and must never disappear.
fn classify(e: &Expr) -> Option<Call<'_>> {
    if e.diagnostic().is_some() {
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
    if method.as_str() != "bytes" || recv.ty.as_ref() != Some(&Ty::Str) {
        return None;
    }
    Some(if !args.is_empty() {
        Call::Unsupported("String#bytes accepts no positional arguments")
    } else if block.as_ref().is_some_and(|block| {
        block.diagnostic().is_some()
            || !matches!(
                &*block.node,
                ExprNode::Lit {
                    value: Literal::Nil
                }
            )
    }) {
        Call::Unsupported("String#bytes with a block is not implemented for this target")
    } else {
        Call::Array(recv)
    })
}

/// Let the project guard recognize this implemented no-block primitive and
/// Ruby-family emission canonicalize literal &nil without accepting unrelated
/// or effectful Proc forwarding.
pub fn materializes_array(e: &Expr) -> bool {
    matches!(classify(e), Some(Call::Array(_)))
}

/// Classify the complete call before a backend separates its block from its
/// receiver. Omitted blocks and literal `&nil` materialize an array; supplied
/// blocks remain explicit refusals. Ruby/Spinel keep native byte behavior and
/// canonicalize literal &nil to an omitted block through the same classifier.
pub fn emit(
    e: &Expr,
    target: Target,
    emit_receiver: impl FnOnce(&Expr) -> String,
) -> Option<String> {
    Some(match classify(e)? {
        Call::Unsupported(detail) => crate::emit::diagnostics::report_unsupported(
            e.span,
            target.name(),
            "String#bytes",
            detail,
        ),
        Call::Array(recv) => render(target, &emit_receiver(recv)),
    })
}

/// Preserve one receiver evaluation and one encoding pass. Unicode-native
/// strings expose their UTF-8 representation; Ruby/Spinel retain raw bytes.
pub fn render(target: Target, recv: &str) -> String {
    match target {
        Target::Rust => {
            format!("({recv}).as_bytes().iter().map(|byte| i64::from(*byte)).collect::<Vec<i64>>()")
        }
        Target::TypeScript => format!("Array.from(new TextEncoder().encode({recv}))"),
        Target::Crystal => format!("({recv}).bytes.map {{ |byte| byte.to_i64 }}"),
        Target::Python => format!("list(({recv}).encode(\"utf-8\"))"),
        Target::Kotlin => format!(
            "({recv}).toByteArray(Charsets.UTF_8).map {{ byte -> (byte.toInt() and 255).toLong() }}.toMutableList()"
        ),
        Target::Swift => format!("({recv}).utf8.map {{ Int($0) }}"),
        Target::CSharp => {
            format!("System.Text.Encoding.UTF8.GetBytes({recv}).Select(b => (long)b).ToList()")
        }
        Target::Go => format!(
            "func(text string) []int64 {{ out := make([]int64, len(text)); for i := 0; i < len(text); i++ {{ out[i] = int64(text[i]) }}; return out }}({recv})"
        ),
        Target::Elixir => format!(":binary.bin_to_list({recv})"),
    }
}
