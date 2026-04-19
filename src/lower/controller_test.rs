//! Controller-test IR analysis — shared logic for per-target emitters.
//!
//! Each target emits the same set of Rails test primitives — `get`,
//! `post`, `patch`, `delete`, `assert_response`, `assert_select`,
//! `assert_difference`, `assert_equal`, etc. The IR-shape recognition
//! is target-independent; only the rendered output differs. This
//! module captures that recognition once so new targets become render
//! tables over `ControllerTestSend` / `UrlHelperCall` rather than
//! re-implementing the same pattern matches.
//!
//! Pairs with `src/lower/controller.rs` — same lift pattern, applied
//! to the test-body surface instead of the action-body surface.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// What kind of `assert_select` shape the test has. `assert_select`
/// takes 1-3 positional args plus an optional block; the four shapes
/// below cover everything the scaffold blog's tests use.
#[derive(Debug)]
pub enum AssertSelectKind<'a> {
    /// `assert_select "selector"` — opening-tag / attribute check.
    SelectorOnly,
    /// `assert_select "selector" do body end` — selector match plus
    /// block of nested inner-body assertions.
    SelectorBlock(&'a Expr),
    /// `assert_select "selector", "text"` or a Send with no args
    /// (treated as a text-returning expression).
    Text(&'a Expr),
    /// `assert_select "selector", minimum: N` — count assertion.
    Minimum(&'a Expr),
}

/// Classification of a test-body `Send` statement. `None` means the
/// caller falls through to a generic expression render.
#[derive(Debug)]
pub enum ControllerTestSend<'a> {
    /// `get <url>` → fetch a page and bind `resp`.
    HttpGet { url: &'a Expr },
    /// `post/patch <url>, params: { ... }`.
    HttpWrite {
        method: &'a str,
        url: &'a Expr,
        params: Option<&'a Expr>,
    },
    /// `delete <url>`.
    HttpDelete { url: &'a Expr },

    /// `assert_response :success | :unprocessable_entity | <other>`.
    AssertResponse { sym: Symbol },
    /// `assert_redirected_to <url_expr>`.
    AssertRedirectedTo { url: &'a Expr },
    /// `assert_select "<sel>"[, <text>|<opts>] [do <body> end]`.
    AssertSelect {
        selector: &'a Expr,
        kind: AssertSelectKind<'a>,
    },
    /// `assert_difference("<expr>"[, delta]) { body }` /
    /// `assert_no_difference("<expr>") { body }`.
    AssertDifference {
        method: &'a str,
        count_expr: String,
        delta: i64,
        block: Option<&'a Expr>,
    },
    /// `assert_equal <expected>, <actual>`.
    AssertEqual {
        expected: &'a Expr,
        actual: &'a Expr,
    },
}

/// Recognize `get/post/patch/delete/assert_*` shapes in a test-body
/// `Send`. Returns `None` if the method isn't one of the recognized
/// test primitives — the caller falls back to its generic Send
/// rendering (method call with emitted args).
pub fn classify_controller_test_send<'a>(
    method: &'a str,
    args: &'a [Expr],
    block: Option<&'a Expr>,
) -> Option<ControllerTestSend<'a>> {
    match method {
        "get" => Some(ControllerTestSend::HttpGet {
            url: args.first()?,
        }),
        "post" | "patch" => Some(ControllerTestSend::HttpWrite {
            method,
            url: args.first()?,
            params: extract_params_kwarg(args),
        }),
        "delete" => Some(ControllerTestSend::HttpDelete {
            url: args.first()?,
        }),
        "assert_response" => {
            let sym = args.first().and_then(as_sym_literal)?;
            Some(ControllerTestSend::AssertResponse { sym })
        }
        "assert_redirected_to" => Some(ControllerTestSend::AssertRedirectedTo {
            url: args.first()?,
        }),
        "assert_select" => {
            let selector = args.first()?;
            let kind = classify_assert_select(args, block);
            Some(ControllerTestSend::AssertSelect { selector, kind })
        }
        "assert_difference" | "assert_no_difference" => {
            let count_expr = args
                .first()
                .and_then(|e| match &*e.node {
                    ExprNode::Lit {
                        value: Literal::Str { value },
                    } => Some(value.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "Unknown.count".to_string());
            let delta = if method == "assert_no_difference" {
                0
            } else {
                args.get(1)
                    .and_then(|e| match &*e.node {
                        ExprNode::Lit {
                            value: Literal::Int { value },
                        } => Some(*value),
                        _ => None,
                    })
                    .unwrap_or(1)
            };
            Some(ControllerTestSend::AssertDifference {
                method,
                count_expr,
                delta,
                block,
            })
        }
        "assert_equal" => Some(ControllerTestSend::AssertEqual {
            expected: args.first()?,
            actual: args.get(1)?,
        }),
        _ => None,
    }
}

/// Determine which assert_select shape the args + block describe.
/// Precedence: string/expr text → `{ minimum: N }` → block → bare.
pub fn classify_assert_select<'a>(
    args: &'a [Expr],
    block: Option<&'a Expr>,
) -> AssertSelectKind<'a> {
    if let Some(second) = args.get(1) {
        match &*second.node {
            ExprNode::Lit {
                value: Literal::Str { .. },
            } => return AssertSelectKind::Text(second),
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    if let ExprNode::Lit {
                        value: Literal::Sym { value: key },
                    } = &*k.node
                    {
                        if key.as_str() == "minimum" {
                            return AssertSelectKind::Minimum(v);
                        }
                    }
                }
            }
            _ => return AssertSelectKind::Text(second),
        }
    }
    if let Some(b) = block {
        return AssertSelectKind::SelectorBlock(b);
    }
    AssertSelectKind::SelectorOnly
}

fn extract_params_kwarg<'a>(args: &'a [Expr]) -> Option<&'a Expr> {
    args.iter().skip(1).find_map(|a| match &*a.node {
        ExprNode::Hash { entries, .. } => entries.iter().find_map(|(k, v)| {
            if let ExprNode::Lit {
                value: Literal::Sym { value },
            } = &*k.node
            {
                if value.as_str() == "params" {
                    return Some(v);
                }
            }
            None
        }),
        _ => None,
    })
}

fn as_sym_literal(e: &Expr) -> Option<Symbol> {
    if let ExprNode::Lit {
        value: Literal::Sym { value },
    } = &*e.node
    {
        return Some(value.clone());
    }
    None
}

/// A single arg to a Rails URL helper, classified by emit shape.
/// Target emitters render each variant per their runtime conventions.
#[derive(Debug)]
pub enum UrlArg<'a> {
    /// Ivar / local var — emit as `<name>.id` (model.id accessor).
    IvarOrVarId(&'a str),
    /// `Model.last` chained on a Const — emit as e.g.
    /// `Model::last().unwrap().id` (Rust) / `Model.last()!.id` (TS).
    ModelLast(Symbol),
    /// Anything else — fall back to the emitter's generic expression
    /// render.
    Raw(&'a Expr),
}

/// Classified URL-helper call. Helpers look like
/// `articles_url`, `article_url(@article)`, etc. — strip `_url`/`_path`
/// suffix, collect positional args, classify each by emit shape.
#[derive(Debug)]
pub struct UrlHelperCall<'a> {
    pub helper_base: String,
    pub args: Vec<UrlArg<'a>>,
}

/// Recognize `<name>_url(...)` / `<name>_path(...)` calls. Returns
/// `None` when the expression isn't a bare Send matching the helper
/// naming convention.
pub fn classify_url_expr<'a>(expr: &'a Expr) -> Option<UrlHelperCall<'a>> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        ..
    } = &*expr.node
    else {
        return None;
    };
    let base = method
        .as_str()
        .strip_suffix("_url")
        .or_else(|| method.as_str().strip_suffix("_path"))?;
    let classified = args
        .iter()
        .map(|a| match &*a.node {
            ExprNode::Ivar { name } => UrlArg::IvarOrVarId(name.as_str()),
            ExprNode::Var { name, .. } => UrlArg::IvarOrVarId(name.as_str()),
            ExprNode::Send {
                recv: Some(r),
                method: m,
                args: inner,
                ..
            } if m.as_str() == "last" && inner.is_empty() => {
                if let ExprNode::Const { path } = &*r.node {
                    let cls = path.last().cloned().unwrap_or_else(|| Symbol::from(""));
                    UrlArg::ModelLast(cls)
                } else {
                    UrlArg::Raw(a)
                }
            }
            _ => UrlArg::Raw(a),
        })
        .collect();
    Some(UrlHelperCall {
        helper_base: base.to_string(),
        args: classified,
    })
}

/// Flatten a Ruby-shape params Hash into a list of
/// `(bracket_key, Value-Expr)` pairs. `{ article: { title: "X" } }`
/// becomes `[("article[title]", <Str X expr>)]`. The emitter renders
/// each value expression per its target conventions.
pub fn flatten_params_pairs<'a>(
    expr: &'a Expr,
    scope: Option<&str>,
) -> Vec<(String, &'a Expr)> {
    let mut pairs: Vec<(String, &'a Expr)> = Vec::new();
    if let ExprNode::Hash { entries, .. } = &*expr.node {
        for (k, v) in entries {
            let key_name = match &*k.node {
                ExprNode::Lit {
                    value: Literal::Sym { value },
                } => value.as_str().to_string(),
                ExprNode::Lit {
                    value: Literal::Str { value },
                } => value.clone(),
                _ => continue,
            };
            let full_key = match scope {
                Some(s) => format!("{s}[{key_name}]"),
                None => key_name.clone(),
            };
            if let ExprNode::Hash { .. } = &*v.node {
                pairs.extend(flatten_params_pairs(v, Some(&full_key)));
            } else {
                pairs.push((full_key, v));
            }
        }
    }
    pairs
}

/// Flatten a test body into a statement sequence. If the body is a
/// single `Seq`, unwrap it; otherwise return it as a singleton.
/// Matches the convention that Rails test bodies are always Seq at
/// the top level but Prism may elide singleton Seqs.
pub fn test_body_stmts(body: &Expr) -> Vec<&Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    }
}
