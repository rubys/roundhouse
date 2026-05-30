//! Thread-local diagnostic sink for the emit path.
//!
//! Emit's `emit_expr`/`emit_send`/`emit_body` functions are pure
//! `fn(&Expr) -> String` and recurse deeply, so threading a
//! `&mut Vec<Diagnostic>` through every signature would touch hundreds
//! of call sites for a value used only at the rare coverage-gap arms.
//! Instead we collect through a thread-local — the same idiom the
//! runtime emit already uses for validation errors and the response
//! object.
//!
//! The lifecycle is scope-based: [`scope`] installs a fresh buffer for
//! the duration of one transpile (saving/restoring any outer buffer so
//! nested or sequential transpiles don't bleed into each other), runs
//! the closure, and returns the collected diagnostics alongside the
//! result. Outside any active scope, [`push`] is a no-op — a catch-all
//! arm still emits its `raise` stub, the diagnostic is simply not
//! retained. Phase 3 wraps the top-level emit drivers in [`scope`] so
//! real runs surface the whole inventory.
//!
//! [`report_unsupported`] is the one-call helper the catch-alls invoke:
//! it pushes a [`Diagnostic::unsupported`] *and* returns the
//! target-appropriate `raise`/`panic`/`throw` stub string, so a gap
//! both self-reports and degrades to a runtime raise at that single
//! site (reusing the existing `Expr.diagnostic → raise` semantics).

use std::cell::RefCell;

use crate::diagnostic::Diagnostic;
use crate::ident::Symbol;

thread_local! {
    /// Active emit diagnostic buffer. `None` when no [`scope`] is
    /// installed (pushes drop); `Some` for the duration of a scope.
    static EMIT_DIAGS: RefCell<Option<Vec<Diagnostic>>> = const { RefCell::new(None) };
}

/// Run `f` with a fresh emit diagnostic buffer installed, returning its
/// result paired with every diagnostic pushed during the call. Any
/// outer buffer is saved and restored, so scopes nest and run in
/// sequence without leaking diagnostics into one another.
pub fn scope<T>(f: impl FnOnce() -> T) -> (T, Vec<Diagnostic>) {
    let prev = EMIT_DIAGS.with(|c| c.borrow_mut().replace(Vec::new()));
    let result = f();
    let collected = EMIT_DIAGS.with(|c| {
        std::mem::replace(&mut *c.borrow_mut(), prev).unwrap_or_default()
    });
    (result, collected)
}

/// Push a diagnostic into the active emit buffer. A no-op when called
/// outside a [`scope`] — the caller's degrade stub still stands, the
/// diagnostic is just not collected.
pub fn push(d: Diagnostic) {
    EMIT_DIAGS.with(|c| {
        if let Some(buf) = c.borrow_mut().as_mut() {
            buf.push(d);
        }
    });
}

/// Report an unsupported construct from within an emitter: push a
/// structured [`Diagnostic::unsupported`] for `target`, and return the
/// degrade stub string to emit at the site. `target` is the concrete
/// backend name (`"go"`, `"rust"`, …) — it both labels the diagnostic
/// and selects the stub syntax. `detail` is collected on the diagnostic
/// for the inventory but kept out of the terse runtime stub.
pub fn report_unsupported(
    target: &str,
    construct: impl Into<Symbol>,
    detail: impl Into<String>,
) -> String {
    let target_sym = Symbol::from(target);
    let construct = construct.into();
    push(Diagnostic::unsupported(
        Some(target_sym.clone()),
        construct.clone(),
        detail,
    ));
    let text = Diagnostic::unsupported_text(Some(&target_sym), &construct);
    StubStyle::for_target(target).render(&text)
}

/// How a target spells "raise at runtime" — used to render the degrade
/// stub. Each variant reproduces exactly the raise-equivalent that
/// target's emitter already drops for `Expr.diagnostic` annotations, so
/// unsupported-construct stubs are syntactically identical to existing
/// incompatible-binop stubs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StubStyle {
    /// Ruby / Elixir / Crystal: `raise "…"`.
    Raise,
    /// Go: `panic("…")`.
    GoPanic,
    /// Rust: `panic!("…")`.
    RustPanic,
    /// TypeScript expression-position throw: `(() => { throw … })()`.
    TsThrow,
    /// Python expression-position throw via a throwing generator.
    PythonThrow,
}

impl StubStyle {
    /// Stub style for a concrete backend name. Unknown targets fall
    /// back to `Raise` (a dynamic-target-safe default).
    pub fn for_target(target: &str) -> StubStyle {
        match target {
            "rust" | "rust2" => StubStyle::RustPanic,
            "go" | "go2" => StubStyle::GoPanic,
            "typescript" => StubStyle::TsThrow,
            "python" => StubStyle::PythonThrow,
            // ruby, elixir, elixir2, crystal, and anything else.
            _ => StubStyle::Raise,
        }
    }

    /// Render the degrade stub for human text `text`. The emitted
    /// program raises `"roundhouse: <text>"` at the site. The `{:?}`
    /// formatting double-quotes and escapes the literal, which every
    /// supported target accepts.
    pub fn render(&self, text: &str) -> String {
        let msg = format!("roundhouse: {text}");
        match self {
            StubStyle::Raise => format!("raise {msg:?}"),
            StubStyle::GoPanic => format!("panic({msg:?})"),
            StubStyle::RustPanic => format!("panic!({msg:?})"),
            StubStyle::TsThrow => {
                format!("(() => {{ throw new Error({msg:?}); }})()")
            }
            StubStyle::PythonThrow => {
                format!("(_ for _ in ()).throw(TypeError({msg:?}))")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticKind;

    #[test]
    fn scope_collects_pushed_diagnostics() {
        let (stub, diags) = scope(|| report_unsupported("go", "While", "non-tail body"));
        assert_eq!(stub, r#"panic("roundhouse: While not supported (go)")"#);
        assert_eq!(diags.len(), 1);
        match &diags[0].kind {
            DiagnosticKind::Unsupported { target, construct, detail } => {
                assert_eq!(target.as_ref().map(|t| t.as_str()), Some("go"));
                assert_eq!(construct.as_str(), "While");
                assert_eq!(detail, "non-tail body");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn push_outside_scope_is_a_noop() {
        // Must not panic, and must not retain anything for the next scope.
        push(Diagnostic::unsupported(None, "Stray", ""));
        let (_, diags) = scope(|| ());
        assert!(diags.is_empty());
    }

    #[test]
    fn nested_scopes_isolate() {
        let (outer_inner_diags, outer_diags) = scope(|| {
            push(Diagnostic::unsupported(Some(Symbol::from("rust")), "Outer", ""));
            let (_, inner) = scope(|| {
                push(Diagnostic::unsupported(Some(Symbol::from("rust")), "Inner", ""));
            });
            inner
        });
        // Inner scope captured only its own push.
        assert_eq!(outer_inner_diags.len(), 1);
        assert!(matches!(
            &outer_inner_diags[0].kind,
            DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Inner"
        ));
        // Outer scope captured only its own push, not the inner one.
        assert_eq!(outer_diags.len(), 1);
        assert!(matches!(
            &outer_diags[0].kind,
            DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "Outer"
        ));
    }

    #[test]
    fn stub_style_per_target_matches_existing_raise_equivalents() {
        assert_eq!(StubStyle::for_target("rust2"), StubStyle::RustPanic);
        assert_eq!(StubStyle::for_target("go2"), StubStyle::GoPanic);
        assert_eq!(StubStyle::for_target("typescript"), StubStyle::TsThrow);
        assert_eq!(StubStyle::for_target("python"), StubStyle::PythonThrow);
        assert_eq!(StubStyle::for_target("elixir2"), StubStyle::Raise);

        assert_eq!(StubStyle::Raise.render("x"), r#"raise "roundhouse: x""#);
        assert_eq!(StubStyle::RustPanic.render("x"), r#"panic!("roundhouse: x")"#);
        assert_eq!(
            StubStyle::TsThrow.render("x"),
            r#"(() => { throw new Error("roundhouse: x"); })()"#
        );
        assert_eq!(
            StubStyle::PythonThrow.render("x"),
            r#"(_ for _ in ()).throw(TypeError("roundhouse: x"))"#
        );
    }
}
