//! Rewritten at ingest, not in the lowering, so the analyzer types each
//! prop expression where it is written and the synthesized share methods
//! have callers during analysis.
//!
//! Also owns `InertiaRails.configure` extraction from initializers.

use std::collections::HashMap;
use std::path::Path;

use crate::App;
use crate::app::InertiaConfig;
use crate::dialect::ControllerBodyItem;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::vfs::Vfs;

use super::controller_macro_synth::{append_private_actions, push_after_filter};
use super::{IngestError, IngestResult, survey};

#[derive(Clone, Copy, PartialEq)]
enum PropKind {
    Eager,
    Optional,
    Deferred,
}

struct Prop {
    key: String,
    kind: PropKind,
    group: String,
    value: Expr,
}

/// Result of recognizing an `InertiaRails.optional` / `.defer` wrapper
/// (or deciding the value is a plain eager expression).
enum PropValue {
    Plain(Expr),
    Wrapped {
        kind: PropKind,
        group: String,
        body: Expr,
    },
    Unsupported,
}

/// Scan `config/initializers` for `InertiaRails.configure` and accumulate
/// supported options into one config (matching the gem's single object).
pub fn load_inertia_config<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
) -> IngestResult<Option<InertiaConfig>> {
    let init_dir = dir.join("config/initializers");
    let mut inertia = InertiaConfig::default();
    let mut found = false;
    if vfs.is_dir(&init_dir) {
        for entry in super::app::read_rb_files(vfs, &init_dir)? {
            if let Ok(bytes) = vfs.read(&entry) {
                let file = entry.display().to_string();
                if extract_inertia_config(&bytes, &file, &mut inertia) {
                    found = true;
                }
            }
        }
    }
    Ok(if found { Some(inertia) } else { None })
}

/// A non-literal value is a survey gap rather than an evaluated guess:
/// the emitted app would otherwise run with the gem's default and no
/// diagnostic would say so.
fn extract_inertia_config(source: &[u8], file: &str, config: &mut InertiaConfig) -> bool {
    let src = String::from_utf8_lossy(source);
    if !src.contains("InertiaRails") {
        return false;
    }
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(program) = root.as_program_node() else {
        return false;
    };
    let mut found = false;
    for stmt in super::app::initializer_statements(&program) {
        let Some(call) = stmt.as_call_node() else { continue };
        if super::util::constant_id_str(&call.name()) != "configure" {
            continue;
        }
        let Some(recv) = call.receiver() else { continue };
        if !matches!(
            super::app::constant_text(&recv, &src).as_deref(),
            Some("InertiaRails" | "::InertiaRails")
        ) {
            continue;
        }
        let named = call.block().and_then(|b| b.as_block_node()).and_then(|block| {
            let param = block
                .parameters()
                .and_then(|p| p.as_block_parameters_node())
                .and_then(|p| p.parameters())
                .and_then(|p| p.requireds().iter().next())
                .and_then(|p| p.as_required_parameter_node())?;
            Some((block, param))
        });
        let Some((block, param)) = named else {
            survey::record(&IngestError::Unsupported {
                file: file.to_string(),
                message: "InertiaRails.configure: a configuration without a block that names its parameter (`do |config|`) is not read; its options are not applied".to_string(),
            });
            found = true;
            continue;
        };
        let param = super::util::constant_id_str(&param.name());
        found = true;
        let Some(body) = block.body() else { continue };
        // Not read even partially: the main statements' assignments hold
        // only if nothing raises, and the rescue clauses' only if something
        // does, so neither is the configuration on its own.
        let Some(body) = body.as_statements_node() else {
            survey::record(&IngestError::Unsupported {
                file: file.to_string(),
                message: "InertiaRails.configure: a block with `rescue` or `ensure` is not read; its options are not applied".to_string(),
            });
            continue;
        };
        for inner in body.body().iter() {
            let loc = inner.location();
            let text = &src[loc.start_offset()..loc.end_offset()];
            let gap = |why: &str| {
                survey::record(&IngestError::Unsupported {
                    file: file.to_string(),
                    message: format!(
                        "InertiaRails.configure: {why} (`{text}`); the emitted app uses the gem's default"
                    ),
                });
            };
            let Some(assign) = inner.as_call_node() else {
                gap("not an option assignment");
                continue;
            };
            let on_param = assign
                .receiver()
                .and_then(|r| r.as_local_variable_read_node())
                .is_some_and(|r| super::util::constant_id_str(&r.name()) == param);
            let name = super::util::constant_id_str(&assign.name()).to_string();
            let args: Vec<_> = assign
                .arguments()
                .map(|a| a.arguments().iter().collect())
                .unwrap_or_default();
            let (true, Some(option), [value]) =
                (on_param, name.strip_suffix('='), args.as_slice())
            else {
                gap("not an option assignment");
                continue;
            };
            let boolean = if value.as_true_node().is_some() {
                Some(true)
            } else if value.as_false_node().is_some() {
                Some(false)
            } else {
                None
            };
            match (option, boolean) {
                ("encrypt_history", Some(b)) => config.encrypt_history = b,
                ("always_include_errors_hash", Some(b)) => config.always_include_errors_hash = b,
                ("use_script_element_for_initial_page", Some(b)) => {
                    config.use_script_element_for_initial_page = b
                }
                // Accepted rather than reported: it names the attribute of
                // SSR head tags, and without SSR nothing on the server reads it.
                ("use_data_inertia_head_attribute", Some(_)) => {}
                ("version", _) => {
                    if let Some(s) = value.as_string_node() {
                        config.version = String::from_utf8_lossy(s.unescaped()).into_owned();
                    } else {
                        gap("version is not a String literal");
                    }
                }
                _ => gap("option not modeled"),
            }
        }
    }
    found
}

pub fn lower_inertia(app: &mut App) {
    let shares = expand_shares(app);
    let config = app.inertia.clone().unwrap_or_default();
    let mut uses_inertia = !shares.is_empty();
    let parents: HashMap<ClassId, Option<ClassId>> = app
        .controllers
        .iter()
        .map(|c| (c.name.clone(), c.parent.clone()))
        .collect();
    for controller in &mut app.controllers {
        let mut chain = vec![controller.name.clone()];
        let mut cur = controller.parent.clone();
        let mut seen = std::collections::HashSet::new();
        seen.insert(controller.name.clone());
        while let Some(p) = cur {
            if !seen.insert(p.clone()) {
                break;
            }
            cur = parents.get(&p).cloned().flatten();
            chain.push(p);
        }
        let share_methods: Vec<Symbol> = chain
            .iter()
            .rev()
            .filter_map(|c| shares.iter().find(|(n, _)| n == c))
            .flat_map(|(_, methods)| methods.iter().cloned())
            .collect();
        let mut ctx = RenderCtx {
            config: &config,
            share_methods: &share_methods,
            used: false,
        };
        for item in &mut controller.body {
            if let ControllerBodyItem::Action { action, .. } = item {
                desugar(&mut action.body, &mut ctx);
            }
        }
        uses_inertia |= ctx.used;
    }
    if uses_inertia {
        install_see_other(app);
    }
}

fn expand_shares(app: &mut App) -> Vec<(ClassId, Vec<Symbol>)> {
    let mut out = Vec::new();
    for controller in &mut app.controllers {
        let base = format!(
            "inertia_share_{}",
            crate::naming::snake_case(&controller.name.0.as_str().replace("::", ""))
        );
        let mut sources = String::new();
        let mut methods: Vec<Symbol> = Vec::new();
        let mut consumed: Vec<usize> = Vec::new();
        for (i, item) in controller.body.iter().enumerate() {
            let ControllerBodyItem::Unknown { expr, .. } = item else {
                continue;
            };
            let Some(props) = share_props(expr) else {
                if is_inertia_share(expr) {
                    record_gap(
                        expr.span,
                        "`inertia_share` with a block, `if:`/`unless:`/`only:`/`except:`, a non-literal key or an InertiaRails prop type other than `optional`/`defer` is not lowered; its shared props are dropped from the output",
                    );
                }
                continue;
            };
            let name = if methods.is_empty() {
                base.clone()
            } else {
                format!("{base}_{}", methods.len() + 1)
            };
            sources.push_str(&share_method_source(&name, &props));
            methods.push(Symbol::from(name.as_str()));
            consumed.push(i);
        }
        if methods.is_empty() {
            continue;
        }
        if !append_private_actions(controller, "<inertia_share>", &sources) {
            // `inertia_share` is CONSUMED_CONTROLLER_MACROS — without a gap
            // here the Unknown stays and the generic reporter skips it.
            for &i in &consumed {
                if let ControllerBodyItem::Unknown { expr, .. } = &controller.body[i] {
                    record_gap(
                        expr.span,
                        "`inertia_share` could not be re-ingested; its shared props are dropped from the output",
                    );
                }
            }
            continue;
        }
        for i in consumed.into_iter().rev() {
            controller.body.remove(i);
        }
        out.push((controller.name.clone(), methods));
    }
    out
}

fn share_props(call: &Expr) -> Option<Vec<Prop>> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        block: None,
        ..
    } = &*call.node
    else {
        return None;
    };
    if method.as_str() != "inertia_share" {
        return None;
    }
    let [hash] = args.as_slice() else {
        return None;
    };
    let ExprNode::Hash { entries, .. } = &*hash.node else {
        return None;
    };
    let mut props = Vec::new();
    for (k, v) in entries {
        let key = literal_key(k)?;
        if matches!(key.as_str(), "if" | "unless" | "only" | "except") {
            return None;
        }
        let Ok((kind, group, value)) = decode_prop_value(v) else {
            return None;
        };
        props.push(Prop {
            key,
            kind,
            group,
            value,
        });
    }
    Some(props)
}

fn is_inertia_share(call: &Expr) -> bool {
    matches!(
        &*call.node,
        ExprNode::Send { recv: None, method, .. } if method.as_str() == "inertia_share"
    )
}

fn share_method_source(name: &str, props: &[Prop]) -> String {
    let span = Span::synthetic();
    let mut src = format!("  def {name}\n");
    for p in props {
        let stmt = prop_stmt(span, p, true);
        let emitted = crate::emit::ruby::expr::emit_expr(&stmt);
        for line in emitted.lines() {
            src.push_str("    ");
            src.push_str(line);
            src.push('\n');
        }
    }
    src.push_str("  end\n");
    src
}

/// Share and `render inertia:` props share one decode: empty-arity
/// `-> { … }` unwraps to the body (inertia_rails evaluates Proc props),
/// then `InertiaRails.optional` / `.defer`, else a plain eager value.
fn decode_prop_value(v: &Expr) -> Result<(PropKind, String, Expr), ()> {
    if let ExprNode::Lambda {
        params,
        rest_param: None,
        block_param: None,
        body,
        ..
    } = &*v.node
        && params.is_empty()
    {
        return Ok((PropKind::Eager, String::new(), body.clone()));
    }
    match classify_prop_value(v) {
        PropValue::Plain(e) => Ok((PropKind::Eager, String::new(), e)),
        PropValue::Wrapped { kind, group, body } => Ok((kind, group, body)),
        PropValue::Unsupported => Err(()),
    }
}

fn classify_prop_value(v: &Expr) -> PropValue {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        block,
        ..
    } = &*v.node
    else {
        return PropValue::Plain(v.clone());
    };
    let ExprNode::Const { path } = &*recv.node else {
        return PropValue::Plain(v.clone());
    };
    if path.len() != 1 || path[0].as_str() != "InertiaRails" {
        return PropValue::Plain(v.clone());
    }
    let body = match block.as_ref().map(|b| &*b.node) {
        Some(ExprNode::Lambda { params, body, .. }) if params.is_empty() => body.clone(),
        _ => return PropValue::Unsupported,
    };
    match method.as_str() {
        "optional" if args.is_empty() => PropValue::Wrapped {
            kind: PropKind::Optional,
            group: String::new(),
            body,
        },
        "defer" => {
            let mut group = "default".to_string();
            match args.as_slice() {
                [] => {}
                [opts] => {
                    let ExprNode::Hash { entries, .. } = &*opts.node else {
                        return PropValue::Unsupported;
                    };
                    for (k, gv) in entries {
                        let Some(key) = literal_key(k) else {
                            return PropValue::Unsupported;
                        };
                        if key.as_str() != "group" {
                            return PropValue::Unsupported;
                        }
                        let ExprNode::Lit {
                            value: Literal::Str { value },
                        } = &*gv.node
                        else {
                            return PropValue::Unsupported;
                        };
                        group = value.clone();
                    }
                }
                _ => return PropValue::Unsupported,
            }
            PropValue::Wrapped {
                kind: PropKind::Deferred,
                group,
                body,
            }
        }
        _ => PropValue::Unsupported,
    }
}

fn literal_key(k: &Expr) -> Option<String> {
    match &*k.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value.as_str().to_string()),
        ExprNode::Lit {
            value: Literal::Str { value },
        } => Some(value.clone()),
        _ => None,
    }
}

struct RenderCtx<'a> {
    config: &'a InertiaConfig,
    share_methods: &'a [Symbol],
    used: bool,
}

fn desugar(e: &mut Expr, ctx: &mut RenderCtx) {
    if let Some(replacement) = render_replacement(e, ctx).or_else(|| redirect_replacement(e)) {
        ctx.used = true;
        *e = replacement;
        return;
    }
    e.node.for_each_child_mut(&mut |c| desugar(c, ctx));
}

/// One prop: optional `mark_shared`, then `if guard; prop; end`.
fn prop_stmt(span: Span, p: &Prop, shared: bool) -> Expr {
    let page = || call(span, None, "inertia_page", Vec::new());
    let key = str_lit(span, &p.key);
    let guard = match p.kind {
        PropKind::Eager => call(span, Some(page()), "eager?", vec![key.clone()]),
        PropKind::Optional => call(span, Some(page()), "optional?", vec![key.clone()]),
        PropKind::Deferred => {
            call(
                span,
                Some(page()),
                "deferred?",
                vec![key.clone(), str_lit(span, &p.group)],
            )
        }
    };
    let set = call(span, Some(page()), "prop", vec![key.clone(), p.value.clone()]);
    let gated = if_expr(span, guard, set, nil(span));
    if shared {
        let mark = call(span, Some(page()), "mark_shared", vec![key]);
        Expr::new(
            span,
            ExprNode::Seq {
                exprs: vec![mark, gated],
            },
        )
    } else {
        gated
    }
}

fn render_replacement(e: &Expr, ctx: &RenderCtx) -> Option<Expr> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        block: None,
        ..
    } = &*e.node
    else {
        return None;
    };
    if method.as_str() != "render" {
        return None;
    }
    let [opts] = args.as_slice() else {
        return None;
    };
    let ExprNode::Hash {
        entries,
        kwargs: true,
    } = &*opts.node
    else {
        return None;
    };
    let component = entries
        .iter()
        .find(|(k, _)| literal_key(k).as_deref() == Some("inertia"))?;
    let span = e.span;
    let unsupported = |why: &str| {
        record_gap(
            span,
            &format!("`render inertia:` {why}; the call is left as written"),
        );
        None
    };
    let ExprNode::Lit {
        value: Literal::Str { value: component },
    } = &*component.1.node
    else {
        return unsupported("with a component that is not a String literal");
    };
    let mut props: Vec<Prop> = Vec::new();
    let mut status: Option<Expr> = None;
    for (k, v) in entries {
        match literal_key(k).as_deref() {
            Some("inertia") => {}
            Some("status") => status = Some(v.clone()),
            Some("props") => {
                let ExprNode::Hash { entries: pairs, .. } = &*v.node else {
                    return unsupported("with `props:` that is not a Hash literal");
                };
                for (pk, pv) in pairs {
                    let Some(key) = literal_key(pk) else {
                        return unsupported("with a prop key that is not a literal");
                    };
                    let Ok((kind, group, value)) = decode_prop_value(pv) else {
                        return unsupported(
                            "with an InertiaRails prop type other than `optional`/`defer`",
                        );
                    };
                    props.push(Prop {
                        key,
                        kind,
                        group,
                        value,
                    });
                }
            }
            _ => return unsupported("with an option other than `props:`/`status:`"),
        }
    }
    let status = status.unwrap_or_else(|| sym(span, "ok"));
    let config = ctx.config;
    let mut stmts = vec![call(
        span,
        None,
        "inertia_begin",
        vec![
            str_lit(span, component),
            str_lit(span, &config.version),
            bool_lit(span, config.encrypt_history),
            bool_lit(span, config.always_include_errors_hash),
        ],
    )];
    for m in ctx.share_methods {
        stmts.push(call(span, None, m.as_str(), Vec::new()));
    }
    for p in props {
        stmts.push(prop_stmt(span, &p, false));
    }
    let status_kw = |s: &Expr| (sym(span, "status"), s.clone());
    let json = call(
        span,
        None,
        "render_inertia_json",
        vec![kwargs(span, vec![status_kw(&status)])],
    );
    // Hard-coded `"application"` until controller layout resolution is
    // modeled (gem default is the controller's layout). Omitting `layout:`
    // here emits a bare page with no shell in the current runtime.
    let html = call(
        span,
        None,
        "render",
        vec![
            call(
                span,
                None,
                "inertia_root_html",
                vec![bool_lit(span, config.use_script_element_for_initial_page)],
            ),
            kwargs(
                span,
                vec![
                    (sym(span, "layout"), str_lit(span, "application")),
                    status_kw(&status),
                ],
            ),
        ],
    );
    stmts.push(if_expr(
        span,
        call(span, None, "inertia_request?", Vec::new()),
        json,
        html,
    ));
    Some(Expr::new(span, ExprNode::Seq { exprs: stmts }))
}

fn redirect_replacement(e: &Expr) -> Option<Expr> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        block: None,
        parenthesized,
    } = &*e.node
    else {
        return None;
    };
    if !matches!(method.as_str(), "redirect_to" | "redirect_back_or_to") {
        return None;
    }
    let (opts, rest) = args.split_last()?;
    let ExprNode::Hash {
        entries,
        kwargs: true,
    } = &*opts.node
    else {
        return None;
    };
    let (_, inertia) = entries
        .iter()
        .find(|(k, _)| literal_key(k).as_deref() == Some("inertia"))?;
    let ExprNode::Hash {
        entries: inertia_entries,
        ..
    } = &*inertia.node
    else {
        record_gap(
            e.span,
            "`inertia:` redirect option that is not a Hash literal; the call is left as written",
        );
        return None;
    };
    let mut errors = None;
    for (k, v) in inertia_entries {
        if literal_key(k).as_deref() != Some("errors") {
            record_gap(
                e.span,
                "`inertia:` redirect option other than `errors:`; the call is left as written",
            );
            return None;
        }
        errors = Some(v.clone());
    }
    let span = e.span;
    let mut stmts = Vec::new();
    if let Some(errors) = errors {
        // `to_hash`, not the value itself, because the gem stores
        // `errors.to_hash`; a Hash literal is passed as written.
        let errors = if matches!(&*errors.node, ExprNode::Hash { .. }) {
            errors
        } else {
            call(errors.span, Some(errors), "to_hash", Vec::new())
        };
        stmts.push(call(span, None, "inertia_errors", vec![errors]));
    }
    let kept: Vec<(Expr, Expr)> = entries
        .iter()
        .filter(|(k, _)| literal_key(k).as_deref() != Some("inertia"))
        .cloned()
        .collect();
    let mut new_args = rest.to_vec();
    if !kept.is_empty() {
        new_args.push(kwargs(span, kept));
    }
    stmts.push(Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: method.clone(),
            args: new_args,
            block: None,
            parenthesized: *parenthesized,
        },
    ));
    Some(Expr::new(span, ExprNode::Seq { exprs: stmts }))
}

/// An `after_action` on the root controllers rather than a change in each
/// dispatcher (the two scaffold `main.rb` files and the test harness),
/// which is where inertia_rails' middleware would sit.
fn install_see_other(app: &mut App) {
    let names: Vec<ClassId> = app.controllers.iter().map(|c| c.name.clone()).collect();
    for controller in &mut app.controllers {
        let is_root = controller
            .parent
            .as_ref()
            .is_none_or(|p| !names.contains(p));
        if !is_root {
            continue;
        }
        let source = "  def inertia_see_other\n    inertia_redirect_see_other\n  end\n";
        if !append_private_actions(controller, "<inertia_see_other>", source) {
            continue;
        }
        push_after_filter(controller, "inertia_see_other");
    }
}

fn record_gap(span: Span, message: &str) {
    let file = super::sources::path_of(span.file).unwrap_or_default();
    survey::record(&IngestError::Unsupported {
        file,
        message: message.to_string(),
    });
}

fn call(span: Span, recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn if_expr(span: Span, cond: Expr, then_branch: Expr, else_branch: Expr) -> Expr {
    Expr::new(
        span,
        ExprNode::If {
            cond,
            then_branch,
            else_branch,
        },
    )
}

fn kwargs(span: Span, entries: Vec<(Expr, Expr)>) -> Expr {
    Expr::new(span, ExprNode::Hash { entries, kwargs: true })
}

fn str_lit(span: Span, s: &str) -> Expr {
    Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Str {
                value: s.to_string(),
            },
        },
    )
}

fn bool_lit(span: Span, value: bool) -> Expr {
    Expr::new(span, ExprNode::Lit {
        value: Literal::Bool { value },
    })
}

fn sym(span: Span, s: &str) -> Expr {
    Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Sym {
                value: Symbol::from(s),
            },
        },
    )
}

fn nil(span: Span) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Nil })
}
