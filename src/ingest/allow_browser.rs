//! `allow_browser versions: …, block: …` → a `before_action` and the
//! private method it runs.
//!
//! Rails 7.2+ ships the macro on every controller (the generated
//! ApplicationController writes `allow_browser versions: :modern`), and
//! campfire writes it inside a concern's `included do` with its own
//! floors and its own fallback page:
//!
//! ```ruby
//! allow_browser versions: VERSIONS, block: -> { render template: "sessions/incompatible_browser" }
//! ```
//!
//! actionpack's definition is one line — `before_action -> {
//! allow_browser(versions:, block:) }, **options` — where the private
//! `allow_browser` asks `BrowserBlocker` and, when blocked, runs the
//! block (or renders the static 406 page). That is what this generates,
//! as real methods the way `channel_callbacks` does, because every
//! consumer downstream reads methods and filters and none reads a
//! class-body macro:
//!
//! ```ruby
//! def allow_browser
//!   if ActionController::BrowserBlocker.blocked?(request.user_agent, { "safari" => "17.2", … })
//!     render template: "sessions/incompatible_browser"
//!   end
//! end
//! ```
//!
//! plus `before_action :allow_browser` with the macro's `only:` /
//! `except:`. Written into the concern's methods and its `included do`
//! filters BEFORE the concern splice, so an includer receives both the
//! way it receives any concern filter.
//!
//! THE CONCERN FORM ONLY, for now. The generator's own line —
//! `allow_browser versions: :modern` straight in ApplicationController
//! — is in every Rails 7.2+ app, real-blog included, and real-blog is
//! emitted for twelve targets of which only the ruby family's request
//! answers `user_agent`. Lowering that form today would put a call no
//! other runtime defines into every one of their trees; it is a
//! controller-body `unknown_calls` entry as before, dropped with the
//! emit's ledger warning, until `request.user_agent` exists on the
//! other runtimes. `take_from_controller_body` is the half that will
//! turn on then; campfire's concern reaches only the ruby lanes.
//!
//! THE FLOORS TRAVEL AS STRINGS. `versions:` is a Hash literal of
//! Float, Integer and `false` (or `:modern`, Rails' own table, or a
//! constant the module declares that resolves to such a literal). The
//! runtime's `BrowserBlocker.blocked?` compares a floor only as a
//! version string, and a `Hash[String, String]` is a shape every
//! target's emit answers where a Float|Integer|false value is not
//! (`user_agent.rb` and `browser_blocker.rb` are shared runtime). A
//! `versions:` this cannot read — a method call, a constant with no
//! literal behind it — leaves the macro where it was, which the emit's
//! dropped-call ledger reports as before.
//!
//! The fallback block's body is re-rendered to source and ingested
//! again inside the generated method, so `render template: "…"`
//! reaches the same render lowering an action's would.

use crate::dialect::{Controller, ControllerBodyItem, Filter, FilterKind, LibraryClass};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

/// The macro's arguments, read off the call.
struct Gate {
    /// `{ "safari" => "17.2", … }` as Ruby source, or the runtime's
    /// `:modern` table by name.
    floors_src: String,
    /// The fallback's body as Ruby source.
    fallback_src: String,
    only: Vec<Symbol>,
    except: Vec<Symbol>,
}

pub fn lower_allow_browser(app: &mut crate::App) {
    // Concerns: the macro inside `included do`.
    let mut concern_filters: Vec<(ClassId, Filter)> = Vec::new();
    let mut generated: Vec<(usize, Vec<crate::dialect::MethodDef>)> = Vec::new();
    for (i, lc) in app.library_classes.iter_mut().enumerate() {
        if !lc.is_module {
            continue;
        }
        let Some(gate) = take_from_included_block(lc) else { continue };
        let src = format!("module {}\n{}end\n", lc.name.0.as_str(), method_source(&gate));
        let methods = match crate::ingest::ingest_library_classes(src.as_bytes(), "<allow_browser>") {
            Ok(classes) => classes.into_iter().flat_map(|c| c.methods).collect(),
            Err(err) => {
                super::survey::record(&err);
                continue;
            }
        };
        generated.push((i, methods));
        concern_filters.push((lc.name.clone(), filter(&gate)));
    }
    for (i, methods) in generated {
        app.library_classes[i].methods.extend(methods);
    }
    for (module, f) in concern_filters {
        app.concern_filters.entry(module).or_default().push(f);
    }

    // Controllers: the macro in the class body. Off until every target's
    // request answers `user_agent` — see the module doc.
    const GENERATOR_FORM: bool = false;
    if !GENERATOR_FORM {
        return;
    }
    for controller in &mut app.controllers {
        let Some(gate) = take_from_controller_body(controller) else { continue };
        let src = format!(
            "class {} < ApplicationController\n  private\n{}end\n",
            controller.name.0.as_str(),
            method_source(&gate)
        );
        let parsed = match crate::ingest::controller::ingest_controller(src.as_bytes(), "<allow_browser>") {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(err) => {
                super::survey::record(&err);
                continue;
            }
        };
        let action = parsed.body.into_iter().find_map(|item| match item {
            ControllerBodyItem::Action { action, .. } => Some(action),
            _ => None,
        });
        let Some(action) = action else { continue };
        let has_private_marker = controller
            .body
            .iter()
            .any(|item| matches!(item, ControllerBodyItem::PrivateMarker { .. }));
        if !has_private_marker {
            controller.body.push(ControllerBodyItem::PrivateMarker {
                leading_comments: Vec::new(),
                leading_blank_line: true,
            });
        }
        controller.body.push(ControllerBodyItem::Action {
            action,
            leading_comments: Vec::new(),
            leading_blank_line: true,
        });
    }
}

/// ```ruby
///   def allow_browser
///     if ActionController::BrowserBlocker.blocked?(request.user_agent, <floors>)
///       <fallback>
///     end
///   end
/// ```
fn method_source(gate: &Gate) -> String {
    format!(
        "  def allow_browser\n    if ActionController::BrowserBlocker.blocked?(request.user_agent, {})\n      {}\n    end\n  end\n",
        gate.floors_src, gate.fallback_src
    )
}

fn filter(gate: &Gate) -> Filter {
    Filter {
        target_span: crate::span::Span::synthetic(),
        kind: FilterKind::Before,
        target: Symbol::from("allow_browser"),
        from_concern: None,
        only: gate.only.clone(),
        except: gate.except.clone(),
        only_style: Default::default(),
        except_style: Default::default(),
        if_cond: None,
        unless_cond: None,
        if_cond_expr: None,
        unless_cond_expr: None,
        block: None,
    }
}

/// Pull the macro out of the concern's `included do … end` (an
/// `unknown_calls` entry), leaving the block's other statements. An
/// emptied block is dropped whole.
fn take_from_included_block(lc: &mut LibraryClass) -> Option<Gate> {
    let constants = lc.constants.clone();
    let mut found: Option<Gate> = None;
    lc.unknown_calls.retain_mut(|call| {
        if found.is_some() {
            return true;
        }
        let ExprNode::Send { recv: None, method, block: Some(block), .. } = &mut *call.node else {
            return true;
        };
        if method.as_str() != "included" {
            return true;
        }
        let ExprNode::Lambda { body, .. } = &mut *block.node else { return true };
        let stmts: Vec<Expr> = match &*body.node {
            ExprNode::Seq { exprs } => exprs.clone(),
            _ => vec![body.clone()],
        };
        let mut kept: Vec<Expr> = Vec::new();
        for stmt in stmts {
            if found.is_none() {
                if let Some(gate) = gate_from_call(&stmt, &constants) {
                    found = Some(gate);
                    continue;
                }
            }
            kept.push(stmt);
        }
        if found.is_none() {
            return true;
        }
        if kept.is_empty() {
            return false;
        }
        *body = Expr::new(body.span, ExprNode::Seq { exprs: kept });
        true
    });
    found
}

/// Replace the macro in a controller's body with its `before_action`.
fn take_from_controller_body(controller: &mut Controller) -> Option<Gate> {
    let constants: Vec<(Symbol, Expr)> = Vec::new();
    let mut found: Option<Gate> = None;
    for item in controller.body.iter_mut() {
        if found.is_some() {
            break;
        }
        let ControllerBodyItem::Unknown { expr, leading_comments, leading_blank_line } = item else {
            continue;
        };
        let Some(gate) = gate_from_call(expr, &constants) else { continue };
        let f = filter(&gate);
        *item = ControllerBodyItem::Filter {
            filter: f,
            leading_comments: std::mem::take(leading_comments),
            leading_blank_line: *leading_blank_line,
        };
        found = Some(gate);
    }
    found
}

/// `allow_browser versions: V, block: -> { … }, only: […]` → its parts,
/// or None for any call this is not, or a `versions:` it cannot read.
fn gate_from_call(call: &Expr, constants: &[(Symbol, Expr)]) -> Option<Gate> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else {
        return None;
    };
    if method.as_str() != "allow_browser" {
        return None;
    }
    let [opts] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, kwargs: true } = &*opts.node else { return None };
    let mut floors_src: Option<String> = None;
    let mut fallback_src: Option<String> = None;
    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
        match key.as_str() {
            "versions" => floors_src = Some(floors_source(v, constants)?),
            "block" => {
                let ExprNode::Lambda { body, .. } = &*v.node else { return None };
                fallback_src = Some(crate::emit::ruby::expr::emit_expr(body));
            }
            "only" => only = symbol_list(v)?,
            "except" => except = symbol_list(v)?,
            _ => return None,
        }
    }
    Some(Gate {
        floors_src: floors_src?,
        // actionpack's default renders public/406-unsupported-browser.html
        // with that status; the page is the app's to ship and the
        // status is the contract.
        fallback_src: fallback_src.unwrap_or_else(|| "head(:not_acceptable)".to_string()),
        only,
        except,
    })
}

/// The `versions:` value as the runtime's `Hash[String, String]`
/// literal in Ruby source: `:modern` names the runtime's table, a Hash
/// literal stringifies each floor, and a bare constant resolves through
/// the declaring module's constants.
fn floors_source(v: &Expr, constants: &[(Symbol, Expr)]) -> Option<String> {
    match &*v.node {
        ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "modern" => {
            Some("ActionController::BrowserBlocker::MODERN".to_string())
        }
        ExprNode::Const { path } => {
            let [name] = path.as_slice() else { return None };
            let (_, value) = constants.iter().find(|(n, _)| n == name)?;
            floors_source(value, &[])
        }
        ExprNode::Hash { entries, .. } => {
            let mut pairs: Vec<String> = Vec::new();
            for (k, floor) in entries {
                let browser = match &*k.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                    ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                    _ => return None,
                };
                let floor = match &*floor.node {
                    ExprNode::Lit { value: Literal::Int { value } } => value.to_string(),
                    ExprNode::Lit { value: Literal::Float { value } } => {
                        let s = value.to_string();
                        if s.contains('.') { s } else { format!("{s}.0") }
                    }
                    ExprNode::Lit { value: Literal::Bool { value: false } } => "false".to_string(),
                    ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                    _ => return None,
                };
                pairs.push(format!("{:?} => {:?}", browser, floor));
            }
            Some(format!("{{ {} }}", pairs.join(", ")))
        }
        _ => None,
    }
}

fn symbol_list(v: &Expr) -> Option<Vec<Symbol>> {
    match &*v.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(vec![value.clone()]),
        ExprNode::Array { elements, .. } => elements
            .iter()
            .map(|e| match &*e.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}
