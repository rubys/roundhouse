//! Shared controller-body walker — the dispatch skeleton every
//! target's emit path shares, lifted into one place after six
//! parallel implementations confirmed the shape.
//!
//! The walker's ten lines of dispatch (Seq / Assign with Create-
//! pattern or default / If with Update-pattern or default / Send
//! via render table / other via expr) are structurally identical
//! across all six targets — only the emitted syntax differs. The
//! `CtrlWalker` trait captures the structure; each target's impl
//! fills in its own render methods.
//!
//! Per-target Assign-level macros (Go's `ModelFind` nil-guard;
//! Elixir's post-save id rebind) live inside each target's leaf
//! methods — there's no pre_assign hook, since those nuances
//! fit naturally inside `write_assign` and `write_if`.

use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use crate::lower::NestedParent;

/// Target-neutral facts every emitter's render table needs. All
/// fields are borrowed from the `LoweredAction` that produced the
/// walker run; the struct is purely a convenience bundle.
pub struct WalkCtx<'a> {
    pub known_models: &'a [Symbol],
    pub model_class: &'a str,
    pub resource: &'a str,
    pub parent: Option<&'a NestedParent>,
    pub permitted: &'a [String],
}

/// Mutable walker state threaded through the dispatch.
///
/// - `uses_context`: set by any render that touches `context.*` /
///   the per-target request-context variable. Callers pick
///   `_context` vs `context` in the signature based on the final
///   value, so warning-as-error toolchains stay happy.
/// - `last_local`: most recently bound local's name. Implicit
///   render passes it by reference to the view fn.
/// - `last_local_is_new`: true only when `last_local` came from a
///   Create-pattern expansion. Elixir's post-save id rebind gates
///   on this so Update flows (where the local came from
///   `ModelFind`) don't get a spurious rebind.
#[derive(Default)]
pub struct WalkState {
    pub uses_context: bool,
    pub last_local: Option<String>,
    pub last_local_is_new: bool,
}

impl WalkState {
    pub fn new() -> Self { Self::default() }
}

/// Classification of a Send rendering. `Response` is a complete
/// response-producing fragment (render / redirect_to / head);
/// `Expr` is an ordinary expression fragment.
pub enum Stmt {
    Response(String),
    Expr(String),
}

/// A target's controller-body walker. The default `walk_stmt`
/// handles the entire dispatch tree; targets implement the leaf
/// methods for their idiomatic syntax.
///
/// Method naming: `render_*` returns a `String` fragment; `write_*`
/// writes (possibly multi-line) output directly to an `out` buffer
/// at the given indent. Multi-line forms (Create expansion,
/// if/else blocks) use `write_*` so they can manage their own
/// line breaks.
pub trait CtrlWalker<'a>: Sized {
    fn ctx(&self) -> &WalkCtx<'a>;
    fn state_mut(&mut self) -> &mut WalkState;

    /// Indent unit per nesting level (e.g. `"  "`, `"    "`, `"\t"`).
    fn indent_unit(&self) -> &'static str;

    /// Emit a plain local binding: `const x = rhs;` / `let x = rhs`
    /// / `x := rhs` / `x = rhs` etc. `value` has NOT been pre-
    /// classified — targets may inspect it for per-target shapes
    /// (Go's `ModelFind` gets a post-bind nil-guard here).
    fn write_assign(&mut self, name: &str, value: &Expr, indent: &str, out: &mut String);

    /// Emit the Create-scaffold expansion: binding to `new Model()`
    /// + per-field assigns keyed off `self.ctx().permitted`. Called
    /// when the Assign RHS matched `model_new_with_strong_params`.
    fn write_create_expansion(
        &mut self,
        var_name: &str,
        class: &str,
        indent: &str,
        out: &mut String,
    );

    /// Emit `if cond { then } else { else }` in target syntax.
    /// Implementations typically render `cond` via `render_expr`,
    /// then recurse into branches via `self.walk_stmt(…)`.
    fn write_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
        indent: &str,
        depth: usize,
        is_tail: bool,
        out: &mut String,
    );

    /// Emit the Update-scaffold rewrite: per-field conditional
    /// assigns hoisted out of the if-cond, then `if x.save { then
    /// } else { else }` with the save-check substituted for the
    /// original `x.update(post_params)` call. `recv` is the
    /// update-target — emit its render-expr form inside.
    fn write_update_if(
        &mut self,
        recv: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
        indent: &str,
        depth: usize,
        is_tail: bool,
        out: &mut String,
    );

    /// Emit a response-terminal statement. `is_tail` marks whether
    /// the statement is the function's trailing expression — Rust
    /// omits `return` and the semicolon in that case; other targets
    /// always prepend `return`.
    fn write_response_stmt(&mut self, r: &str, is_tail: bool, indent: &str, out: &mut String);

    /// Emit an expression fragment as a `;`-terminated (or newline-
    /// terminated) statement. Called for Send nodes that classified
    /// as `Stmt::Expr` and for any non-control-flow leaf node.
    fn write_expr_stmt(&mut self, s: &str, indent: &str, out: &mut String);

    /// Render an Expr as an expression fragment. Target-specific:
    /// handles Send via the render-send-stmt table; defers to the
    /// target's `emit_expr` for literals/consts/etc.
    fn render_expr(&mut self, expr: &Expr) -> String;

    /// Render a Send through the target's SendKind render table.
    /// `None` → the target doesn't classify this Send and wants
    /// the walker to fall through to the generic expression path.
    fn render_send_stmt(
        &mut self,
        recv: Option<&Expr>,
        method: &str,
        args: &[Expr],
        block: Option<&Expr>,
    ) -> Option<Stmt>;

    /// Top-level entry: walk a normalized action body, producing
    /// the target's inside-function-body text. Caller is expected
    /// to wrap the result in the fn signature + closing brace.
    fn walk_action_body(&mut self, body: &Expr) -> String {
        let mut out = String::new();
        self.walk_stmt(body, &mut out, 1, true);
        out
    }

    /// Statement walker. Shared dispatch across all targets —
    /// targets shouldn't need to override this. `is_tail` threads
    /// through so Seq's last element knows it's at the tail.
    fn walk_stmt(&mut self, expr: &Expr, out: &mut String, depth: usize, is_tail: bool) {
        let indent = self.indent_unit().repeat(depth);
        match &*expr.node {
            ExprNode::Seq { exprs } => {
                let last_idx = exprs.len().saturating_sub(1);
                for (i, e) in exprs.iter().enumerate() {
                    self.walk_stmt(e, out, depth, is_tail && i == last_idx);
                }
            }
            ExprNode::Assign { target: LValue::Var { name, .. }, value }
            | ExprNode::Assign { target: LValue::Ivar { name }, value } => {
                let (known_models, resource) =
                    (self.ctx().known_models, self.ctx().resource);
                if let Some(class) = crate::lower::model_new_with_strong_params(
                    value, known_models, resource,
                ) {
                    self.write_create_expansion(
                        name.as_str(), class.as_str(), &indent, out,
                    );
                    let st = self.state_mut();
                    st.last_local = Some(name.as_str().to_string());
                    st.last_local_is_new = true;
                    return;
                }
                self.write_assign(name.as_str(), value, &indent, out);
                let st = self.state_mut();
                st.last_local = Some(name.as_str().to_string());
                st.last_local_is_new = false;
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                let resource = self.ctx().resource;
                if let Some(recv) =
                    crate::lower::update_with_strong_params(cond, resource)
                {
                    self.write_update_if(
                        recv, then_branch, else_branch, &indent, depth, is_tail, out,
                    );
                } else {
                    self.write_if(
                        cond, then_branch, else_branch, &indent, depth, is_tail, out,
                    );
                }
            }
            ExprNode::Send { recv, method, args, block, .. } => {
                match self.render_send_stmt(
                    recv.as_ref(), method.as_str(), args, block.as_ref(),
                ) {
                    Some(Stmt::Response(r)) => {
                        self.write_response_stmt(&r, is_tail, &indent, out);
                    }
                    Some(Stmt::Expr(s)) => {
                        self.write_expr_stmt(&s, &indent, out);
                    }
                    None => {
                        let s = self.render_expr(expr);
                        self.write_expr_stmt(&s, &indent, out);
                    }
                }
            }
            _ => {
                let s = self.render_expr(expr);
                if !s.is_empty() {
                    self.write_expr_stmt(&s, &indent, out);
                }
            }
        }
    }
}
