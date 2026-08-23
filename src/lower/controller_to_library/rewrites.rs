//! Action-body rewrite passes. Each `rewrite_*` / `expand_*` is an
//! independent IR-to-IR transform composed in declared order by
//! `lower_action_body` in `mod.rs`. Runs after `unwrap_respond_to` /
//! `synthesize_implicit_render` so the synthesized symbol-form render
//! shows up here as a plain `Send`.

use std::collections::{BTreeMap, HashSet};

use crate::dialect::Action;
use crate::expr::{ArrayStyle, Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

use super::params::{ParamsSpec, ParamsSpecs};
use super::util::map_expr;

// ---------------------------------------------------------------------------
// Render-template-as-Views-call rewrite. Spinel doesn't have Rails'
// implicit-render-of-eponymous-template; every render goes through an
// explicit `render(Views::<Module>.<method>(<args>))` call. This pass
// handles two source shapes uniformly:
//
//   - `render :show` (synthesized by the upstream pass for actions with
//     no terminal) → `render(Views::Articles.show(@article))`.
//   - `render :new, status: :unprocessable_entity` (explicit, in
//     create's else branch after unwrap_respond_to) →
//     `render(Views::Articles.new(@article), status: :unprocessable_entity)`.
//
// `ivars` is the precomputed scope: every `@x = ...` assignment in the
// action body PLUS every filter target that fires for this action. The
// view-method call gets all of them as positional args, in the order
// they appear in scope. View-side parameter names that don't match
// here are a follow-on lowerer's problem.
// ---------------------------------------------------------------------------

/// `partial: "users/sidebars/rooms/shared", locals: { room: room }` →
/// `Views::Users::Sidebars::Rooms.shared(room)`, bound against the
/// partial's DEF-SITE contract: the record by the partial's record-arg
/// convention, closure ivars as `@<name>` (a same-named local wins),
/// extras from `locals:` (nil-filled, trailing-trimmed).
///
/// Shared by the render rewrite and the broadcast rewrite — a broadcast
/// that names a partial renders exactly what `render partial:` renders,
/// and two copies of this binding would be two chances to disagree
/// about a partial's argument order.
///
/// `module_name` is the controller's own view module, used only for a
/// bare (slash-free) partial name. `None` declines rather than guessing.
pub(super) fn partial_view_call(
    partial: &str,
    locals: &[(Symbol, Expr)],
    module_name: Option<&str>,
    partials: &super::PartialMap,
    span: Span,
) -> Option<Expr> {
    partial_view_call_with_record(partial, locals, None, module_name, partials, span)
}

/// `partial_view_call` with the record supplied directly, for a caller
/// whose record is not in `locals:`.
///
/// `membership.broadcast_prepend_to membership.user, :rooms, partial:
/// "users/sidebars/rooms/direct"` names no locals at all — Rails binds
/// the partial's own local to the RECEIVER. Without the override the
/// record position fell to `nil` and the emit called
/// `Views::…Rooms.direct(nil)`, which renders an empty row instead of
/// failing. `locals:` still wins when it names the record.
pub(super) fn partial_view_call_with_record(
    partial: &str,
    locals: &[(Symbol, Expr)],
    record_override: Option<&Expr>,
    module_name: Option<&str>,
    partials: &super::PartialMap,
    span: Span,
) -> Option<Expr> {
    let (module_dir, base_name) = match partial.rsplit_once('/') {
        // `camelize_PATH`, matching how `partial_call_contracts` spells
        // this key. Plain `camelize` leaves the slashes in
        // (`users/sidebars/rooms` → `Users/sidebars/rooms`) so the
        // contract lookup below missed every NESTED partial and the
        // render stood unrewritten. A single-segment dir spells the same
        // either way, which is why a flat view tree never showed it.
        Some((d, n)) => (
            crate::naming::camelize_path(&crate::naming::snake_case(d)),
            n.to_string(),
        ),
        None => (module_name?.to_string(), partial.to_string()),
    };
    let stem = base_name.trim_start_matches('_').to_string();
    let contract = partials.get(&(module_dir.clone(), stem.clone()))?;
    let lookup = |name: &str| -> Option<Expr> {
        locals
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .map(|(_, v)| v.clone())
    };
    let mut view_args: Vec<Expr> = vec![lookup(&contract.record)
        .or_else(|| record_override.cloned())
        .unwrap_or_else(|| nil_expr(span))];
    for n in &contract.closure {
        view_args.push(lookup(n).unwrap_or_else(|| ivar(n.as_str(), span)));
    }
    let bound: Vec<Option<Expr>> = contract.extras.iter().map(|n| lookup(n)).collect();
    if let Some(last) = bound.iter().rposition(|b| b.is_some()) {
        for b in bound.into_iter().take(last + 1) {
            view_args.push(b.unwrap_or_else(|| nil_expr(span)));
        }
    }
    Some(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&["Views", &module_dir], span)),
            method: crate::lower::view::view_method_name(&stem),
            args: view_args,
            block: None,
            parenthesized: true,
        },
    ))
}

pub(super) fn rewrite_render_to_views(
    expr: &Expr,
    module_name: Option<&str>,
    ivars: &[Symbol],
    view_ivars: &super::ViewIvarMap,
    partials: &super::PartialMap,
    current_action: &str,
) -> Expr {
    let Some(module) = module_name else {
        return expr.clone();
    };
    let module_name_owned = module.to_string();
    let ivars = ivars.to_vec();
    // Controller-context literals every view can read (see
    // extra_params.rs): the current action name and the controller's
    // resource name (`HomeController` → module "Home" → "home").
    let current_action = current_action.to_string();
    let controller_name = crate::naming::snake_case(&module_name_owned);
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, .. }
            if (method.as_str() == "render" || method.as_str() == "render_to_string")
                && !args.is_empty() =>
        {
            // `render_to_string` renders the same view surface but
            // RETURNS the string (no response, no layout) — the rewrite
            // produces the bare `Views::X.y(...)` call, which IS a
            // string. Trailing options (`layout: false`) are dropped.
            let to_string = method.as_str() == "render_to_string";
            // Controller-side partial render: `render partial:
            // "commentbox", layout: false, content_type: …, locals:
            // {comment: c}` (lobsters' reply). Bind against the
            // partial's def-site contract: record by the partial's
            // record-arg convention, closure ivars as `@<name>` (a
            // same-named local wins), extras from locals (nil-filled,
            // trailing-trimmed). `layout:` drops (Rails renders
            // partials without one); other kwargs (content_type:,
            // status:) ride the render.
            if let ExprNode::Hash { entries, kwargs: true } = &*args[0].node {
                let mut partial_name: Option<String> = None;
                let mut locals_entries: Vec<(Symbol, Expr)> = Vec::new();
                let mut rest: Vec<(Expr, Expr)> = Vec::new();
                let mut saw_partial_key = false;
                for (k, v) in entries {
                    let key = match &*k.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
                        _ => "",
                    };
                    match key {
                        "partial" => {
                            saw_partial_key = true;
                            if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                                partial_name = Some(value.clone());
                            }
                        }
                        "locals" => {
                            if let ExprNode::Hash { entries: le, .. } = &*v.node {
                                for (lk, lv) in le {
                                    if let ExprNode::Lit {
                                        value: Literal::Sym { value },
                                    } = &*lk.node
                                    {
                                        locals_entries.push((value.clone(), lv.clone()));
                                    }
                                }
                            }
                        }
                        "layout" => {}
                        _ => rest.push((k.clone(), v.clone())),
                    }
                }
                if let Some(pname) = partial_name {
                    let view_call =
                        partial_view_call(&pname, &locals_entries, module_name, partials, e.span)?;
                    if to_string {
                        return Some(view_call);
                    }
                    let mut new_args = vec![view_call];
                    // Rails renders partials WITHOUT the layout; keep an
                    // explicit `layout: false` marker for the Ruby emit
                    // layout pass to honor-and-strip.
                    rest.push((
                        Expr::new(
                            e.span,
                            ExprNode::Lit {
                                value: Literal::Sym { value: Symbol::from("layout") },
                            },
                        ),
                        Expr::new(
                            e.span,
                            ExprNode::Lit { value: Literal::Bool { value: false } },
                        ),
                    ));
                    new_args.push(Expr::new(
                        args[0].span,
                        ExprNode::Hash { entries: rest, kwargs: true },
                    ));
                    return Some(Expr::new(
                        e.span,
                        ExprNode::Send {
                            recv: None,
                            method: Symbol::from("render"),
                            args: new_args,
                            block: block.clone(),
                            parenthesized: true,
                        },
                    ));
                }
                // A dynamic partial name (`render partial: @x`) at the
                // CONTROLLER seam isn't modeled — leave untouched.
                if saw_partial_key {
                    return None;
                }
            }
            // View name comes from `render :index` (a Symbol first arg),
            // `render "index"` / `render "articles/index"` (a String
            // first arg — slashed form names another view module), or
            // `render action: "index"` / `render action: :index` (a
            // kwarg hash). The kwarg form may also carry other options
            // (status:, …); those leftover entries ride through as
            // `action_hash_extra`. `module_override` is `Some` for the
            // slashed-String form; `Some("")` names a top-level view
            // (`Views.<stem>` — views/not_found.erb-style trees).
            let (view_method, action_hash_extra, module_override): (
                Symbol,
                Vec<Expr>,
                Option<String>,
            ) = match &*args[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => {
                    (value.clone(), Vec::new(), None)
                }
                ExprNode::Lit { value: Literal::Str { value } } => {
                    match value.rsplit_once('/') {
                        Some((dir, stem)) => (
                            Symbol::from(stem),
                            Vec::new(),
                            Some(crate::naming::camelize_path(
                                &crate::naming::snake_case(dir),
                            )),
                        ),
                        // Slashless string: current controller's module,
                        // falling back to a top-level view of that name
                        // when the module has no such template (the
                        // shared-404 idiom).
                        None => {
                            let key =
                                (module_name_owned.clone(), value.to_string());
                            let module = if view_ivars.contains_key(&key) {
                                None
                            } else {
                                Some(String::new())
                            };
                            (Symbol::from(value.as_str()), Vec::new(), module)
                        }
                    }
                }
                ExprNode::Hash { entries, kwargs: true } => {
                    let mut name: Option<Symbol> = None;
                    // (expr, content_type-to-tag) — html carries None
                    // (text/html is the runtime default).
                    let mut body: Option<(Expr, Option<&str>)> = None;
                    let mut rest: Vec<(Expr, Expr)> = Vec::new();
                    for (k, v) in entries {
                        let key = match &*k.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
                            _ => "",
                        };
                        match key {
                            "action" => name = render_target_symbol(v),
                            // `render html:` / `render plain:` — a body
                            // expression, not a template name. Normalized
                            // below to the positional-body form the runtime
                            // render takes. Rails ESCAPES an html: body
                            // unless it's marked safe, and lobsters'
                            // about_controller writes BOTH in one file
                            // (`html: ("<h1>A mystery.")` escaped,
                            // `html: (…).html_safe` not).
                            //
                            // A body that says `.html_safe` right here
                            // answers the escape question at this site, so
                            // the pair cancels: emit the value bare. The
                            // old form leaned on the CRuby overlay's
                            // SafeString to make `html_escape(x.html_safe)`
                            // a pass-through at RUNTIME, which is a
                            // CRuby-only fact — under AOT it was two
                            // separate bugs, `String#html_safe` raising
                            // (every /u visit) and, had it not raised, a
                            // body escaped that Rails leaves alone.
                            "html" => {
                                let safe = unwrap_html_safe(v);
                                let expr = safe.unwrap_or_else(|| html_escape_call(v));
                                body = Some((expr, None))
                            }
                            "plain" => body = Some((v.clone(), Some("text/plain"))),
                            // Inline `render json: <expr>` — the body is
                            // the JSON encoding of the value. Encoding
                            // happens at runtime (`JsonRender.encode`
                            // walks as_json/Hash/Array/Time), because the
                            // value's shape is a runtime fact.
                            "json" => {
                                body = Some((json_render_encode(v), Some("application/json")))
                            }
                            _ => rest.push((k.clone(), v.clone())),
                        }
                    }
                    if name.is_none() {
                        // Non-template render: `render html: X, layout:
                        // "application"` → `render(X, layout: …)` (the
                        // Ruby emit layout pass honors + strips `layout:`);
                        // `render plain: X, status: N` → `render(X,
                        // status: N, content_type: "text/plain")`. A
                        // `layout: false` entry is dropped (no layout is
                        // already the runtime default for a body render).
                        let Some((body_expr, tagged_content_type)) = body else { return None };
                        rest.retain(|(k, v)| {
                            let is_layout_false = matches!(
                                &*k.node,
                                ExprNode::Lit { value: Literal::Sym { value } }
                                    if value.as_str() == "layout"
                            ) && matches!(
                                &*v.node,
                                ExprNode::Lit { value: Literal::Bool { value: false } }
                            );
                            !is_layout_false
                        });
                        let mut new_args = vec![body_expr];
                        if !rest.is_empty() {
                            new_args.push(Expr::new(
                                args[0].span,
                                ExprNode::Hash { entries: rest, kwargs: true },
                            ));
                        }
                        if let Some(ct) = tagged_content_type {
                            merge_or_append_kwarg(&mut new_args, "content_type", ct, e.span);
                        }
                        new_args.extend(args.iter().skip(1).cloned());
                        return Some(Expr::new(
                            e.span,
                            ExprNode::Send {
                                recv: None,
                                method: Symbol::from("render"),
                                args: new_args,
                                block: block.clone(),
                                parenthesized: true,
                            },
                        ));
                    }
                    let Some(n) = name else { return None };
                    let extra = if rest.is_empty() {
                        Vec::new()
                    } else {
                        vec![Expr::new(
                            args[0].span,
                            ExprNode::Hash { entries: rest, kwargs: true },
                        )]
                    };
                    (n, extra, None)
                }
                _ => return None,
            };
            // Which Views module owns the target template — the
            // current controller's by default; the slashed / top-level
            // String forms override (empty string = the root `Views`
            // module).
            let render_module: String =
                module_override.unwrap_or_else(|| module_name_owned.clone());
            // View-driven arg list: an action view's params are exactly the
            // @ivars it reads, so pass `@<name>` for each (matching the
            // generated view signature). Look up by the html action stem
            // (before the `_json` rename below). Falls back to the
            // controller's in-scope ivars when the view isn't in the map
            // (json/jbuilder views, or a render with no matching template).
            // The turbo_stream marker has to be read BEFORE the lookup:
            // its contract is keyed by the format-qualified stem
            // (`create_turbo_stream`), the same name the view lowers to.
            let is_turbo_stream = render_kwargs_have_format(args, "turbo_stream");
            // The contract is keyed by the FORMAT-QUALIFIED stem, so a
            // format marker has to be reflected here or the lookup asks
            // for a template that need not exist. campfire's avatar
            // action is the case that proves it: `users/avatars/` holds
            // `show.svg.erb` and NO `show.html.erb`, so looking up bare
            // `show` missed and the render became MissingTemplate — for a
            // template that is right there.
            let is_svg = render_kwargs_have_format(args, "svg");
            let is_json = render_kwargs_have_format(args, "json");
            let contract_stem = if is_turbo_stream {
                format!("{}_turbo_stream", view_method.as_str())
            } else if is_svg {
                format!("{}_svg", view_method.as_str())
            } else if is_json {
                // Same reason the two above are qualified: the contract
                // is keyed by the FORMAT-QUALIFIED stem. Asking for the
                // bare stem here found the HTML view's contract when
                // there was one and NOTHING when there wasn't — and
                // "nothing" fell through to the controller's own
                // assigned ivars, which for campfire's autocomplete is
                // the empty set (`@page` is assigned inside a runtime
                // method). The call site then passed no arguments to a
                // view that declared one.
                format!("{}_json", view_method.as_str())
            } else {
                view_method.as_str().to_string()
            };
            let contract = view_ivars.get(&(render_module.clone(), contract_stem));
            // A non-json render whose target isn't among the emitted
            // action views means the template doesn't exist in the source
            // tree. Rails raises ActionView::MissingTemplate there — and
            // lobsters' about/privacy actions rescue it as their NORMAL
            // path (hardcoded-fallback pages). Emitting the Views call
            // anyway would be a NoMethodError no rescue catches.
            if contract.is_none() && !is_json {
                return Some(Expr::new(
                    e.span,
                    ExprNode::Raise {
                        value: Expr::new(
                            e.span,
                            ExprNode::Send {
                                recv: Some(const_path(
                                    &["ActionView", "MissingTemplate"],
                                    e.span,
                                )),
                                method: Symbol::from("new"),
                                args: vec![str_lit(e.span, view_method.as_str())],
                                block: None,
                                parenthesized: true,
                            },
                        ),
                    },
                ));
            }
            let resolved_ivars: Vec<Symbol> = contract
                .map(|c| c.ivars.clone())
                .unwrap_or_else(|| ivars.clone());
            // Pass action_name/controller_name literals only to views whose
            // contract records that they reference them (so views that don't
            // get no extra args — no arity mismatch).
            let pass_action_name = contract.map(|c| c.uses_action_name).unwrap_or(false);
            let pass_controller_name = contract.map(|c| c.uses_controller_name).unwrap_or(false);
            // Peek at the trailing kwarg-Hash for a `format: :json`
            // marker that the respond_to flattener planted. If
            // present, route to `<sym>_json` view and tag the outer
            // render with `content_type: "application/json"`. The
            // marker drops out of the rewritten kwargs so it doesn't
            // leak past the lowerer.
            // Format marker planted by the respond_to flattener /
            // implicit-render synthesis. Routes to the format-qualified
            // view method and tags the outer render's content type. The
            // marker drops out of the rewritten kwargs so it doesn't leak
            // past the lowerer.
            // jbuilder templates never reference notice/alert, so the
            // `_json` variant skips the flash extras below. A
            // turbo_stream template is an ordinary ERB view lowered
            // through the normal path, so it takes them like html does.
            let json_format = render_kwargs_have_format(args, "json");
            let (view_method, content_type) =
                if is_turbo_stream {
                    (
                        Symbol::from(format!("{}_turbo_stream", view_method.as_str())),
                        Some("text/vnd.turbo-stream.html"),
                    )
                } else if json_format {
                    (
                        Symbol::from(format!("{}_json", view_method.as_str())),
                        Some("application/json"),
                    )
                } else if is_svg {
                    // campfire's avatar initials. Same shape as the two
                    // above — format-qualified view method, MIME from the
                    // shared table so this arm and `mime_for_format`
                    // cannot disagree about what an svg is.
                    (
                        Symbol::from(format!("{}_svg", view_method.as_str())),
                        Some(crate::lower::controller::body::mime_for_format("svg")),
                    )
                } else {
                    (view_method, None)
                };
            let mut view_args: Vec<Expr> = resolved_ivars
                .iter()
                .map(|n| ivar(n.as_str(), e.span))
                .collect();
            // Every view's signature carries `notice = nil, alert = nil`
            // as trailing extra params (uniform shape; see
            // `view_to_library/extra_params.rs`). Pass `@flash[:notice]`
            // and `@flash[:alert]` from the controller so views that
            // render flash messages receive them. Views that don't
            // reference flash get unused-local args — harmless under
            // any target's emit.
            //
            // The jbuilder lowerer does NOT plumb flash extras (json
            // templates never reference notice/alert), so for the
            // `_json` view variant we pass just the ivars.
            if !json_format {
                view_args.push(flash_lookup(e.span, "notice"));
                view_args.push(flash_lookup(e.span, "alert"));
                if pass_action_name {
                    view_args.push(str_lit(e.span, &current_action));
                }
                if pass_controller_name {
                    view_args.push(str_lit(e.span, &controller_name));
                }
            }
            // Digit-leading stems (`about/404`) carry a `_` prefix on the
            // method — must match the def-site naming in view_to_library.
            let view_method = crate::lower::view::view_method_name(view_method.as_str());
            let view_call = Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(if render_module.is_empty() {
                        const_path(&["Views"], e.span)
                    } else {
                        const_path(&["Views", &render_module], e.span)
                    }),
                    method: view_method,
                    args: view_args,
                    block: None,
                    parenthesized: true,
                },
            );
            if to_string {
                // `render_to_string action: "tree"` — the view call IS
                // the string; response options don't apply.
                return Some(view_call);
            }
            let mut new_args = vec![view_call];
            new_args.extend(action_hash_extra);
            let rest: Vec<Expr> = args
                .iter()
                .skip(1)
                .cloned()
                .filter_map(|a| strip_format_kwarg(&a))
                .collect();
            new_args.extend(rest);
            if let Some(ct) = content_type {
                merge_or_append_kwarg(&mut new_args, "content_type", ct, e.span);
            }
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("render"),
                    args: new_args,
                    block: block.clone(),
                    parenthesized: true,
                },
            ))
        }
        _ => None,
    })
}

/// A String-literal expression (for the action_name/controller_name
/// view args passed from the controller).
fn str_lit(span: Span, s: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } })
}

/// The view name from a `render action:` value — accepts a Symbol
/// (`action: :index`) or a String (`action: "index"`) literal.
fn render_target_symbol(v: &Expr) -> Option<Symbol> {
    match &*v.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(Symbol::from(value.as_str())),
        _ => None,
    }
}

/// True when render's args have a trailing kwarg-Hash whose `format:`
/// entry is `:<fmt>`. The marker is planted by the respond_to
/// flattener for the json branch only — html renders never have it.
fn render_kwargs_have_format(args: &[Expr], fmt: &str) -> bool {
    let Some(last) = args.last() else { return false };
    let ExprNode::Hash { entries, kwargs: true } = &*last.node else {
        return false;
    };
    entries.iter().any(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "format")
            && matches!(&*v.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == fmt)
    })
}

/// Drop the `format:` entry from a trailing kwarg-Hash on a render
/// call. `format:` is the dispatch marker the respond_to flattener
/// planted; it's consumed at lower time and shouldn't leak to the
/// runtime. `location:` passes through (the runtime's render takes
/// `location:` and main.rb ships it as the Location header).
/// Returns `Some(stripped)` if the Hash still has entries, `None` if
/// the strip left it empty (caller drops the now-empty Hash).
fn strip_format_kwarg(arg: &Expr) -> Option<Expr> {
    if let ExprNode::Hash { entries, kwargs: true } = &*arg.node {
        let kept: Vec<(Expr, Expr)> = entries
            .iter()
            .filter(|(k, _)| {
                !matches!(
                    &*k.node,
                    ExprNode::Lit { value: Literal::Sym { value } }
                        if value.as_str() == "format"
                )
            })
            .cloned()
            .collect();
        if kept.is_empty() {
            return None;
        }
        return Some(Expr::new(
            arg.span,
            ExprNode::Hash {
                entries: kept,
                kwargs: true,
            },
        ));
    }
    Some(arg.clone())
}

/// Merge a single `key: <str-value>` entry into the trailing
/// kwarg-Hash of `args`. If args already ends with a kwarg Hash,
/// append the entry to it; otherwise push a fresh kwarg Hash with
/// just this entry. The runtime's `render(body, status:, content_type:)`
/// expects ONE kwargs hash, not multiple.
/// `ActionController::JsonRender.encode(<value>)` — the runtime JSON
/// encoder behind inline `render json: <expr>`. CRuby answers it via
/// the overlay (as_json-aware recursive encode); a strict target whose
/// app reaches this call surfaces an unresolved-constant gap loudly
/// rather than silently rendering html.
/// `ActionView::ViewHelpers.html_escape(<body>)` — the `render html:`
/// escape Rails applies to non-safe bodies. Runtime-dispatched (not
/// folded) because safety is a runtime fact under the CRuby
/// SafeString overlay.
/// Peel a safety mark off a `render html:` body: `<e>.html_safe` and
/// `raw(<e>)` both mean "Rails, don't escape this", and both answer it
/// statically at the call site. Returns the inner expression, or `None`
/// when the body carries no mark and so has to be escaped.
///
/// Only the mark written HERE is peeled. A method that ends in
/// `.html_safe` (lobsters' `Hat#to_html_label`) is a value-level fact
/// no call site can see, and stays the SafeString overlay's problem.
fn unwrap_html_safe(value: &Expr) -> Option<Expr> {
    match &*value.node {
        ExprNode::Send { recv: Some(inner), method, args, block: None, .. }
            if method.as_str() == "html_safe" && args.is_empty() =>
        {
            Some(inner.clone())
        }
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "raw" && args.len() == 1 =>
        {
            Some(args[0].clone())
        }
        _ => None,
    }
}

fn html_escape_call(value: &Expr) -> Expr {
    let recv = Expr::new(
        value.span,
        ExprNode::Const {
            path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")],
        },
    );
    Expr::new(
        value.span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("html_escape"),
            args: vec![value.clone()],
            block: None,
            parenthesized: true,
        },
    )
}

fn json_render_encode(value: &Expr) -> Expr {
    let recv = Expr::new(
        value.span,
        ExprNode::Const {
            path: vec![Symbol::from("ActionController"), Symbol::from("JsonRender")],
        },
    );
    Expr::new(
        value.span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("encode"),
            args: vec![value.clone()],
            block: None,
            parenthesized: true,
        },
    )
}

/// Add `key: value` to the trailing kwargs hash — unless the call site
/// ALREADY passes that key, in which case the author's value stands.
///
/// The content type implied by `plain:`/`html:`/`json:` is a DEFAULT, and
/// Rails lets an explicit `content_type:` override it. Appending
/// unconditionally emitted the key twice; a Ruby hash literal is
/// last-wins, so `render plain: svg, content_type: "image/svg+xml"`
/// shipped as `text/plain` — the author's value written down and then
/// overwritten one entry later, which reads as correct in the emit right
/// up until you check which one Ruby keeps.
fn merge_or_append_kwarg(args: &mut Vec<Expr>, key: &str, value: &str, span: Span) {
    let key_node = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Sym {
                value: Symbol::from(key),
            },
        },
    );
    let val_node = Expr::new(
        span,
        ExprNode::Lit {
            value: Literal::Str {
                value: value.to_string(),
            },
        },
    );
    if let Some(last) = args.last_mut() {
        if let ExprNode::Hash { entries, kwargs: true } = &*last.node {
            let already_set = entries.iter().any(|(k, _)| {
                matches!(
                    &*k.node,
                    ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == key
                )
            });
            if already_set {
                return;
            }
            let mut new_entries = entries.clone();
            new_entries.push((key_node, val_node));
            *last = Expr::new(
                last.span,
                ExprNode::Hash {
                    entries: new_entries,
                    kwargs: true,
                },
            );
            return;
        }
    }
    args.push(Expr::new(
        span,
        ExprNode::Hash {
            entries: vec![(key_node, val_node)],
            kwargs: true,
        },
    ));
}

// ---------------------------------------------------------------------------
// Has-many-through-parent rewrite. Rails' `@article.comments.build(args)`
// and `@article.comments.find(args)` both go through the association
// proxy: build pre-fills the FK from the parent, find scopes the lookup
// to children of the parent. Spinel doesn't have association proxies;
// the parent linkage has to be made explicit at the call site.
//
//   @x = @parent.<assoc>.build(<args>)
//   ─────────────────────────────────────────────────────────────────
//   attrs = <args>.to_h
//   attrs[:<parent>_id] = @parent.id
//   @x = <Singular>.new(attrs)
//
//   @x = @parent.<assoc>.find(<args>)
//   ─────────────────────────────────────────────────────────────────
//   @x = <Singular>.find(<args>)
//   if @x.<parent>_id != @parent.id
//     head(:not_found)
//     return
//   end
//
// One Assign expands to a Seq of multiple Exprs — the outer Seq the
// emitter walks for line-per-statement output flattens implicitly.
// ---------------------------------------------------------------------------

pub(super) fn rewrite_assoc_through_parent_typed(
    expr: &Expr,
    privs: &[Action],
    params_specs: &ParamsSpecs,
) -> Expr {
    let helper_specs = super::params::helper_spec_map(privs, params_specs);
    map_expr(expr, &|e| {
        let ExprNode::Assign { target, value } = &*e.node else {
            return None;
        };
        // Build/Find expansions synthesize `@<lhs>`-shaped follow-on
        // statements, so they require an ivar LHS; the single-statement
        // FindBy rewrite works for local assigns too (`comment =
        // @article.comments.find_by(id: x)`).
        let lhs = match target {
            LValue::Ivar { name } => Some(name),
            _ => None,
        };
        let ExprNode::Send {
            recv: Some(outer_recv),
            method: outer_method,
            args: outer_args,
            block: None,
            ..
        } = &*value.node
        else {
            return None;
        };
        let kind = match outer_method.as_str() {
            "build" => AssocKind::Build,
            // Rails' `create` on an association is `build` + save, over
            // an attribute HASH. A typed params object handed to the
            // runtime's `create` would be indexed like a Hash and
            // NoMethodError, so recognize the composition here — the
            // same reason `<Model>.create(<params>)` needs its own
            // typed factory.
            "create" => AssocKind::Create { bang: false },
            "create!" => AssocKind::Create { bang: true },
            "find" => AssocKind::Find,
            "find_by" => AssocKind::FindBy,
            _ => return None,
        };
        if lhs.is_none() && !matches!(kind, AssocKind::FindBy) {
            return None;
        }
        // `find` needs its id arg; `build` may be bare (`@story.comments.
        // build` — new child for a form, FK pre-filled, no attributes).
        let max_args = 1;
        if outer_args.len() > max_args
            || (outer_args.is_empty() && !matches!(kind, AssocKind::Build))
        {
            return None;
        }
        let ExprNode::Send {
            recv: Some(inner_recv),
            method: assoc_method,
            args: inner_args,
            block: None,
            ..
        } = &*outer_recv.node
        else {
            return None;
        };
        if !inner_args.is_empty() {
            return None;
        }
        let ExprNode::Ivar { name: parent_name } = &*inner_recv.node else {
            return None;
        };
        let model_class = crate::naming::singularize_camelize(assoc_method.as_str());
        let fk = format!("{}_id", parent_name.as_str());
        Some(match kind {
            AssocKind::Build => {
                let lhs = lhs.expect("checked above");
                if outer_args.is_empty() {
                    return Some(expand_build_bare(&model_class, &fk, parent_name, lhs, e.span));
                }
                if let Some(spec) = match_params_helper(&outer_args[0], &helper_specs, params_specs) {
                    return Some(expand_build_typed(
                        &model_class,
                        &fk,
                        parent_name,
                        lhs,
                        &outer_args[0],
                        super::params::model_from_params_name(spec),
                        e.span,
                    ));
                }
                expand_build(&model_class, &fk, parent_name, lhs, &outer_args[0], e.span)
            }
            // Only the TYPED arm is recognized. An attribute-Hash
            // `create` already works through the runtime, and turning it
            // into build+save here would only restate it.
            AssocKind::Create { bang } => {
                let lhs = lhs.expect("checked above");
                let spec = match_params_helper(&outer_args[0], &helper_specs, params_specs)?;
                expand_create_typed(
                    &model_class,
                    &fk,
                    parent_name,
                    lhs,
                    &outer_args[0],
                    super::params::model_from_params_name(spec),
                    bang,
                    e.span,
                )
            }
            AssocKind::Find => {
                let lhs = lhs.expect("checked above");
                expand_find(&model_class, &fk, parent_name, lhs, &outer_args[0], e.span)
            }
            // `<lhs> = @parent.<assoc>.find_by(id: x)` — the assoc scope
            // folds into the conditions hash: `<Class>.find_by(id: x,
            // <fk>: @parent.id)`. One query, nil on missing OR foreign
            // rows — exactly the source's nil-returning contract, so no
            // ownership branch and any LHS shape works.
            AssocKind::FindBy => {
                let ExprNode::Hash { entries, kwargs: true } = &*outer_args[0].node else {
                    return None;
                };
                let mut new_entries = entries.clone();
                new_entries.push((
                    Expr::new(
                        e.span,
                        ExprNode::Lit {
                            value: Literal::Sym { value: Symbol::from(fk.as_str()) },
                        },
                    ),
                    Expr::new(
                        e.span,
                        ExprNode::Send {
                            recv: Some(ivar(parent_name.as_str(), e.span)),
                            method: Symbol::from("id"),
                            args: vec![],
                            block: None,
                            parenthesized: false,
                        },
                    ),
                ));
                Expr::new(
                    e.span,
                    ExprNode::Assign {
                        target: target.clone(),
                        value: Expr::new(
                            e.span,
                            ExprNode::Send {
                                recv: Some(const_path(&[&model_class], e.span)),
                                method: Symbol::from("find_by"),
                                args: vec![Expr::new(
                                    e.span,
                                    ExprNode::Hash { entries: new_entries, kwargs: true },
                                )],
                                block: None,
                                parenthesized: true,
                            },
                        ),
                    },
                )
            }
        })
    })
}

/// True-when-Some: `arg` evaluates to a params object. Returns the spec,
/// so the caller reaches the right class even when the helper's name
/// doesn't match its resource (`bot_params` permits `:user`) or when the
/// resource carries several lists.
///
/// Three spellings, because a call site is free to use any of them and
/// they all arrive at the same typed object:
///   - a bare call to one of this controller's `<x>_params` helpers;
///   - the typed factory the permit chain has ALREADY become — this runs
///     after `rewrite_to_from_raw`, so a chain written inline at the call
///     site reads `<Class>.from_raw(@params)` by now;
///   - either of those under an in-place filter (`.except(:reason)`,
///     `.compact`), which clears presence flags and returns the object
///     itself. lobsters writes exactly that:
///     `@hat_request.update!(params.require(:hat_request)
///     .permit(:hat, :link, :reason).except(:reason))`. Matching only the
///     first spelling left the typed object in a plain `update!`, whose
///     parameter is Rails' attribute Hash — a mismatch no dynamic target
///     notices and every strict one refuses.
fn match_params_helper<'a>(
    arg: &Expr,
    helper_specs: &BTreeMap<Symbol, &'a ParamsSpec>,
    params_specs: &'a ParamsSpecs,
) -> Option<&'a ParamsSpec> {
    if let ExprNode::Send { recv: Some(recv), method, block: None, .. } = &*arg.node {
        if matches!(method.as_str(), "except" | "compact") {
            return match_params_helper(recv, helper_specs, params_specs);
        }
        if method.as_str() == "from_raw" {
            let ExprNode::Const { path } = &*recv.node else { return None };
            let class = path.last()?;
            return params_specs.iter().find(|s| &s.class_id.0 == class);
        }
    }
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*arg.node else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    helper_specs.get(method).copied()
}

enum AssocKind {
    Build,
    /// `@parent.<assoc>.create!(<params>)` — `Build` plus the save it
    /// composes to. `bang` picks `save!` over `save`.
    Create { bang: bool },
    Find,
    FindBy,
}

/// Typed-factory build expansion:
///
/// ```ruby
/// @<lhs> = <Class>.from_params(<arg>)
/// @<lhs>.<fk> = @<parent>.id
/// ```
///
/// `<arg>` is the typed `<resource>_params` helper call (returning
/// `<Resource>Params`); `<Class>.from_params` is the per-model factory
/// added by `model_to_library/schema.rs`. The FK setter follows the
/// model's `attr_writer` for the foreign key column.
pub(super) fn expand_build_typed(
    model_class: &str,
    fk: &str,
    parent: &Symbol,
    lhs: &Symbol,
    arg: &Expr,
    factory: Symbol,
    span: Span,
) -> Expr {
    let from_params_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&[model_class], span)),
            method: factory,
            args: vec![arg.clone()],
            block: None,
            parenthesized: true,
        },
    );
    let lhs_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Ivar { name: lhs.clone() },
            value: from_params_call,
        },
    );

    let parent_id = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(parent.as_str(), span)),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let fk_setter = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(lhs.as_str(), span)),
            method: Symbol::from(format!("{fk}=")),
            args: vec![parent_id],
            block: None,
            parenthesized: false,
        },
    );

    Expr::new(span, ExprNode::Seq { exprs: vec![lhs_assign, fk_setter] })
}

/// `@<lhs> = @<parent>.<assoc>.create!(<params>)` — the typed build,
/// then the save `create` composes. Statement-shaped: this replaces an
/// `Assign` statement, and `flatten_seqs` splices the Seq into the
/// action body (a Seq left in expression position renders as
/// newline-joined lines and binds the wrong value).
#[allow(clippy::too_many_arguments)]
fn expand_create_typed(
    model_class: &str,
    fk: &str,
    parent: &Symbol,
    lhs: &Symbol,
    arg: &Expr,
    factory: Symbol,
    bang: bool,
    span: Span,
) -> Expr {
    let built = expand_build_typed(model_class, fk, parent, lhs, arg, factory, span);
    let ExprNode::Seq { exprs } = &*built.node else { unreachable!("build expands to a Seq") };
    let mut exprs = exprs.clone();
    exprs.push(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(lhs.as_str(), span)),
            method: Symbol::from(if bang { "save!" } else { "save" }),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    ));
    Expr::new(span, ExprNode::Seq { exprs })
}

/// Zero-arg build expansion (`@comment = @story.comments.build`):
///
/// ```ruby
/// @<lhs> = <Class>.new
/// @<lhs>.<fk> = @<parent>.id
/// ```
///
/// No attributes to absorb — just the bare constructor and the parent
/// linkage through the model's foreign-key writer.
fn expand_build_bare(
    model_class: &str,
    fk: &str,
    parent: &Symbol,
    lhs: &Symbol,
    span: Span,
) -> Expr {
    let new_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&[model_class], span)),
            method: Symbol::from("new"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let lhs_assign = Expr::new(
        span,
        ExprNode::Assign { target: LValue::Ivar { name: lhs.clone() }, value: new_call },
    );
    let parent_id = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(parent.as_str(), span)),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let fk_setter = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(lhs.as_str(), span)),
            method: Symbol::from(format!("{fk}=")),
            args: vec![parent_id],
            block: None,
            parenthesized: false,
        },
    );
    Expr::new(span, ExprNode::Seq { exprs: vec![lhs_assign, fk_setter] })
}

pub(super) fn expand_build(
    model_class: &str,
    fk: &str,
    parent: &Symbol,
    lhs: &Symbol,
    arg: &Expr,
    span: Span,
) -> Expr {
    // attrs = <arg>
    // The `.to_h` wrap is added by the params-helpers pass when <arg>
    // is a `<x>_params` call — keeping the two concerns separate avoids
    // double-wrapping if <arg> already has `.to_h` for some reason.
    let attrs_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("attrs") },
            value: arg.clone(),
        },
    );

    // attrs[:<fk>] = @<parent>.id
    let parent_id = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(parent.as_str(), span)),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let fk_sym = Expr::new(
        span,
        ExprNode::Lit { value: Literal::Sym { value: Symbol::from(fk) } },
    );
    let attrs_var = Expr::new(
        span,
        ExprNode::Var { id: VarId(0), name: Symbol::from("attrs") },
    );
    let index_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Index { recv: attrs_var.clone(), index: fk_sym },
            value: parent_id,
        },
    );

    // @<lhs> = <Class>.new(attrs)
    let new_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&[model_class], span)),
            method: Symbol::from("new"),
            args: vec![attrs_var],
            block: None,
            parenthesized: true,
        },
    );
    let final_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Ivar { name: lhs.clone() },
            value: new_call,
        },
    );

    Expr::new(
        span,
        ExprNode::Seq { exprs: vec![attrs_assign, index_assign, final_assign] },
    )
}

pub(super) fn expand_find(
    model_class: &str,
    fk: &str,
    parent: &Symbol,
    lhs: &Symbol,
    arg: &Expr,
    span: Span,
) -> Expr {
    // @<lhs> = <Class>.find(<arg>)
    let find_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&[model_class], span)),
            method: Symbol::from("find"),
            args: vec![arg.clone()],
            block: None,
            parenthesized: true,
        },
    );
    let lhs_assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Ivar { name: lhs.clone() },
            value: find_call,
        },
    );

    // if @<lhs>.<fk> != @<parent>.id; head(:not_found); return; end
    let lhs_fk = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(lhs.as_str(), span)),
            method: Symbol::from(fk),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let parent_id = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(parent.as_str(), span)),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let cond = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(lhs_fk),
            method: Symbol::from("!="),
            args: vec![parent_id],
            block: None,
            parenthesized: false,
        },
    );
    let head_call = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("head"),
            args: vec![Expr::new(
                span,
                ExprNode::Lit { value: Literal::Sym { value: Symbol::from("not_found") } },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let return_stmt = Expr::new(
        span,
        ExprNode::Return {
            value: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        },
    );
    let if_body = Expr::new(
        span,
        ExprNode::Seq { exprs: vec![head_call, return_stmt] },
    );
    let if_stmt = Expr::new(
        span,
        ExprNode::If {
            cond,
            then_branch: if_body,
            else_branch: Expr::new(span, ExprNode::Seq { exprs: vec![] }),
        },
    );

    Expr::new(span, ExprNode::Seq { exprs: vec![lhs_assign, if_stmt] })
}

// ---------------------------------------------------------------------------
// `<Model>.new(<resource>_params)` → `<Model>.from_params(<resource>_params)`.
//
// The typed-factory rewrite that replaces the legacy `.to_h`-wrap pass.
// Now that the `<resource>_params` helper returns a typed `<Resource>Params`
// (synthesized from the controller's `permit` declaration via
// `controller_to_library::params`), the model's `new(attrs: Hash)`
// constructor isn't the right entry point — `from_params` takes the
// typed instance and assigns each permitted field through the named
// accessor.
//
// `create` / `create!` ride the same rewrite. Rails composes them as
// `new(attrs)` + save over an attribute HASH, so a typed params object
// reaching the runtime's `create` would be indexed like a Hash and
// NoMethodError. `create_from_params` is that composition over the typed
// factory (`model_to_library::schema::push_create_from_params_method`).
//
// Match shape: `<Const>.new|create|create!(<bare _params helper call>)`
// where the helper's name is in `privs` and ends with `_params`, and
// `<Const>` is the model the helper's resource names — the condition the
// model lowerer synthesizes the factory under. Any other argument shape
// (Hash literal, Array, …) flows through unchanged.
// ---------------------------------------------------------------------------

pub(super) fn rewrite_model_new_to_from_params(
    expr: &Expr,
    privs: &[Action],
    params_specs: &ParamsSpecs,
) -> Expr {
    // No empty-map bail: `match_params_helper` also recognizes the
    // permit chain written inline at the call site (by now a
    // `<Class>.from_raw(@params)`), and a controller that writes it that
    // way defines no `<x>_params` helper at all.
    let helper_specs = super::params::helper_spec_map(privs, params_specs);
    map_expr(expr, &|e| {
        let ExprNode::Send {
            recv: Some(model_recv),
            method,
            args,
            block: None,
            parenthesized,
        } = &*e.node
        else {
            return None;
        };
        if args.len() != 1 {
            return None;
        }
        // Only rewrite `<Const>.…` shapes — leaves `instance.new(...)`
        // (rare but legal) untouched.
        let ExprNode::Const { path } = &*model_recv.node else {
            return None;
        };
        let spec = match_params_helper(&args[0], &helper_specs, params_specs)?;
        // The typed factories live on the model whose resource this list
        // permits; `Membership.create(boost_params)` has none to reach.
        let model = path.last()?;
        if crate::naming::snake_case(model.as_str()) != spec.resource.as_str() {
            return None;
        }
        let target = match method.as_str() {
            "new" => super::params::model_from_params_name(spec),
            "create" => super::params::model_create_from_params_name(spec, false),
            "create!" => super::params::model_create_from_params_name(spec, true),
            _ => return None,
        };
        Some(Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(model_recv.clone()),
                method: target,
                args: vec![args[0].clone()],
                block: None,
                parenthesized: *parenthesized,
            },
        ))
    })
}

// ---------------------------------------------------------------------------
// `<record>.update(<resource>_params)` → `<record>.update_from_<class>(...)`
// for EVERY permit list.
//
// The model's plain `update` / `update!` take an attribute Hash (Rails'
// own contract). A controller handing one a typed params object is the
// site the lowerer already owns, so it is the site that moves: each
// permit list has a typed update sized to exactly its fields, and this
// rewrite points the call at it by the helper's name. Two lists for one
// resource (campfire's `Users::ProfilesController` adds `:bio` to the
// four fields `UsersController` permits) are unrelated types on every
// strict target and could never have shared a method anyway.
// ---------------------------------------------------------------------------

pub(super) fn rewrite_update_to_typed_variant(
    expr: &Expr,
    privs: &[Action],
    params_specs: &ParamsSpecs,
) -> Expr {
    // No empty-map bail: `match_params_helper` also recognizes the
    // permit chain written inline at the call site (by now a
    // `<Class>.from_raw(@params)`), and a controller that writes it that
    // way defines no `<x>_params` helper at all.
    let helper_specs = super::params::helper_spec_map(privs, params_specs);
    map_expr(expr, &|e| {
        let ExprNode::Send { recv: Some(recv), method, args, block: None, parenthesized } =
            &*e.node
        else {
            return None;
        };
        let bang = match method.as_str() {
            "update" => false,
            "update!" => true,
            _ => return None,
        };
        if args.len() != 1 {
            return None;
        }
        let spec = match_params_helper(&args[0], &helper_specs, params_specs)?;
        Some(Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(recv.clone()),
                method: super::params::model_update_name(spec, bang),
                args: vec![args[0].clone()],
                block: None,
                parenthesized: *parenthesized,
            },
        ))
    })
}

// ---------------------------------------------------------------------------
// `destroy!` → `destroy`. Spinel's runtime model exposes one destroy
// method (raise-on-failure semantics); the bang form has no separate
// behavior to preserve, so the surface gets normalized here. Applies to
// any Send (any recv shape) whose method name is exactly `destroy!`.
// ---------------------------------------------------------------------------

pub(super) fn rewrite_destroy_bang(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv, method, args, block, parenthesized }
            if method.as_str() == "destroy!" =>
        {
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: recv.as_ref().map(rewrite_destroy_bang),
                    method: Symbol::from("destroy"),
                    args: args.iter().map(rewrite_destroy_bang).collect(),
                    block: block.as_ref().map(rewrite_destroy_bang),
                    parenthesized: *parenthesized,
                },
            ))
        }
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// `params` rewrites. Spinel controllers don't have the magic `params`
// method — request params arrive as a plain Hash on `@params`. The two
// Rails 8 idioms encountered here:
//
//   - `params.expect(:id)` → `@params[:id].to_i` (single-symbol form;
//     coerces because @params holds string values from the URL).
//   - `params.expect(post: [ :title, :body ])` → `@params.require(:post)
//     .permit(:title, :body)` (the older strong-params form, which
//     spinel's runtime implements).
//
// And bare `params` references (with no method call) lower to `@params`.
// ---------------------------------------------------------------------------

pub(super) fn rewrite_params(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        // `params.expect(...)` — recognized first so the recv is still
        // the bare `params` Send, not the @params ivar (which would lose
        // the recognition pattern).
        ExprNode::Send { recv: Some(recv), method, args, block, parenthesized }
            if method.as_str() == "expect" && is_bare_params(recv) =>
        {
            Some(rewrite_expect(args, block.as_ref(), *parenthesized, e.span))
        }
        // Bare `params` (no recv, no args, no block) → `@params`.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "params" && args.is_empty() =>
        {
            Some(Expr::new(e.span, ExprNode::Ivar { name: Symbol::from("params") }))
        }
        // `params.require(:r).permit(...)` — NOT ours. That chain belongs
        // to the permit rewrite, and half-rewriting it would leave
        // `.permit` hanging off a plain Hash: one NoMethodError traded
        // for another, with the emit looking fixed. Matched here, ahead
        // of the `require` arm below, and returned with only the bare
        // `params` swapped for `@params` — `map_expr` is top-down and
        // does not recurse into a replacement, so this shields the
        // inner `require` from the arm that follows.
        ExprNode::Send { recv: Some(permit_recv), method, args, block, parenthesized }
            if method.as_str() == "permit" =>
        {
            let ExprNode::Send {
                recv: Some(inner_recv),
                method: inner,
                args: inner_args,
                block: None,
                parenthesized: inner_paren,
            } = &*permit_recv.node
            else {
                return None;
            };
            if inner.as_str() != "require" || !is_bare_params(inner_recv) {
                return None;
            }
            let shielded = Expr::new(
                permit_recv.span,
                ExprNode::Send {
                    recv: Some(Expr::new(
                        inner_recv.span,
                        ExprNode::Ivar { name: Symbol::from("params") },
                    )),
                    method: inner.clone(),
                    args: inner_args.clone(),
                    block: None,
                    parenthesized: *inner_paren,
                },
            );
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(shielded),
                    method: method.clone(),
                    args: args.clone(),
                    block: block.clone(),
                    parenthesized: *parenthesized,
                },
            ))
        }
        // `params.require(:user)[:role]` — an INDEX on the required
        // sub-hash. Rails' `require` answers an
        // `ActionController::Parameters`, whose access is indifferent;
        // `@params` is a plain String-keyed Hash, so a Symbol key finds
        // nothing and the read answers nil. That is a SILENT wrong
        // answer, not an error: campfire's
        // `params.require(:user)[:role].presence_in(%w[…]) || "member"`
        // would have quietly demoted every role change to the default
        // if the `||` had come first — what it did instead was die on
        // `presence_in` for nil, one method later.
        //
        // Matched AHEAD of the standalone-`require` arm and rebuilt
        // whole, because `map_expr` is top-down and does not recurse
        // into a replacement: the index is the outer node, so the
        // require underneath it has to be lowered here or not at all.
        // The type-directed index coercion in the Ruby expr emitter
        // cannot serve this — it keys on a `Hash[String, _]` receiver,
        // and `require_key` is declared to answer a `ParamValue`.
        ExprNode::Send { recv: Some(idx_recv), method, args, block: None, parenthesized }
            if method.as_str() == "[]"
                && args.len() == 1
                && sym_arg(&args[0]).is_some()
                && matches!(&*idx_recv.node,
                    ExprNode::Send { recv: Some(r), method: m, args: a, block: None, .. }
                        if m.as_str() == "require"
                            && a.len() == 1
                            && is_bare_params(r)
                            && sym_or_str_arg(&a[0]).is_some()) =>
        {
            let ExprNode::Send { args: require_args, .. } = &*idx_recv.node else {
                return None;
            };
            let key = sym_or_str_arg(&require_args[0])?;
            let required = Expr::new(
                idx_recv.span,
                ExprNode::Send {
                    recv: Some(const_path(&["Params"], idx_recv.span)),
                    method: Symbol::from("require_key"),
                    args: vec![
                        Expr::new(idx_recv.span, ExprNode::Ivar { name: Symbol::from("params") }),
                        Expr::new(
                            idx_recv.span,
                            ExprNode::Lit { value: Literal::Str { value: key } },
                        ),
                    ],
                    block: None,
                    parenthesized: true,
                },
            );
            let field = sym_arg(&args[0])?;
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(required),
                    method: method.clone(),
                    args: vec![Expr::new(
                        args[0].span,
                        ExprNode::Lit { value: Literal::Str { value: field } },
                    )],
                    block: None,
                    parenthesized: *parenthesized,
                },
            ))
        }
        // `params.require(:url)` STANDING ALONE — Rails' assertion that a
        // parameter was supplied, answering the value. `@params` is a
        // plain Hash and `require` on one reaches Kernel's PRIVATE
        // method, which is how this announced itself: "private method
        // 'require' called for an instance of Hash", in four tests
        // across three files.
        //
        // Only when it is not the receiver of a `.permit` — that chain
        // is the permit rewrite's, and half-rewriting it would leave
        // `.permit` hanging off a Hash, trading one NoMethodError for
        // another while looking fixed.
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "require"
                && args.len() == 1
                && is_bare_params(recv)
                && sym_or_str_arg(&args[0]).is_some() =>
        {
            let key = sym_or_str_arg(&args[0])?;
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(const_path(&["Params"], e.span)),
                    method: Symbol::from("require_key"),
                    args: vec![
                        Expr::new(e.span, ExprNode::Ivar { name: Symbol::from("params") }),
                        Expr::new(
                            e.span,
                            ExprNode::Lit { value: Literal::Str { value: key } },
                        ),
                    ],
                    block: None,
                    parenthesized: true,
                },
            ))
        }
        _ => None,
    })
}

/// The literal name of a `[:sym]` index argument.
fn sym_arg(arg: &Expr) -> Option<String> {
    match &*arg.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    }
}

/// The literal key of a `require(:sym)` / `require("str")` argument.
fn sym_or_str_arg(arg: &Expr) -> Option<String> {
    match &*arg.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// `redirect_to` polymorphic rewrite. Rails' `redirect_to @article` does
// implicit polymorphic resolution to `article_path(@article)`; spinel
// requires the explicit form. The IR-level shape:
//
//   Send { method: "redirect_to", args: [Ivar{name}, ...kwargs] }
//
// becomes:
//
//   Send { method: "redirect_to", args: [
//     Send { recv: Const(RouteHelpers), method: "<name>_path",
//            args: [Send { recv: Ivar{name}, method: "id" }] },
//     ...kwargs
//   ], parenthesized: true }
//
// Only the first positional arg is rewritten; trailing keyword-hash
// args (notice:, status:) pass through unchanged.
// ---------------------------------------------------------------------------

pub(super) fn rewrite_redirect_to(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, .. }
            if method.as_str() == "redirect_to" && !args.is_empty() =>
        {
            // Two recognized first-arg shapes:
            //   - `@x` (Ivar) → wrap as `RouteHelpers.<x>_path(@x.id)`.
            //   - `<x>_path` (no-recv Send ending in _path) → prefix
            //     with RouteHelpers so all redirect_to call sites
            //     render uniformly with the parenthesized form.
            //
            // Other shapes (string URL, hash, …) leave the call alone
            // so we don't accidentally mangle an idiom we don't handle.
            let first = &args[0];
            let new_first = match &*first.node {
                ExprNode::Ivar { name } => polymorphic_path(name, e.span),
                ExprNode::Send { recv: None, method: m, args: m_args, block: m_block, parenthesized }
                    if m.as_str().ends_with("_path") =>
                {
                    Expr::new(
                        first.span,
                        ExprNode::Send {
                            recv: Some(const_path(&["RouteHelpers"], first.span)),
                            method: m.clone(),
                            args: m_args.clone(),
                            block: m_block.clone(),
                            parenthesized: *parenthesized,
                        },
                    )
                }
                _ => return None,
            };
            let mut new_args = vec![new_first];
            new_args.extend(args.iter().skip(1).cloned());
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("redirect_to"),
                    args: new_args,
                    block: block.clone(),
                    parenthesized: true,
                },
            ))
        }
        _ => None,
    })
}

/// `render(:show, …, location: @article)` — Rails' POST-201 idiom.
/// The kwarg value is a polymorphic record reference; rewrite to
/// `RouteHelpers.<singular>_path(@x.id)` so the runtime's
/// `render(body, location: <string>)` sees a path string. Mirrors
/// the `redirect_to @x` polymorphic rewrite below; runs over render
/// Sends specifically (not redirect_to, which has its own pass).
pub(super) fn rewrite_render_location_kwarg(expr: &Expr) -> Expr {
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, parenthesized }
            if method.as_str() == "render" && !args.is_empty() =>
        {
            let new_args: Vec<Expr> = args
                .iter()
                .map(|a| rewrite_location_in_kwargs(a))
                .collect();
            if new_args
                .iter()
                .zip(args.iter())
                .all(|(a, b)| std::ptr::eq(a.node.as_ref(), b.node.as_ref()))
            {
                // No change — let map_expr recurse normally.
                return None;
            }
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: None,
                    method: method.clone(),
                    args: new_args,
                    block: block.clone(),
                    parenthesized: *parenthesized,
                },
            ))
        }
        _ => None,
    })
}

/// If `arg` is a kwarg-Hash with a `location:` entry whose value is
/// a polymorphic record ref (Ivar), rewrite that entry's value to a
/// path-helper call. Other shapes pass through untouched.
fn rewrite_location_in_kwargs(arg: &Expr) -> Expr {
    let ExprNode::Hash { entries, kwargs: true } = &*arg.node else {
        return arg.clone();
    };
    let mut changed = false;
    let new_entries: Vec<(Expr, Expr)> = entries
        .iter()
        .map(|(k, v)| {
            let is_location = matches!(
                &*k.node,
                ExprNode::Lit { value: Literal::Sym { value } }
                    if value.as_str() == "location"
            );
            if !is_location {
                return (k.clone(), v.clone());
            }
            match &*v.node {
                ExprNode::Ivar { name } => {
                    changed = true;
                    (k.clone(), polymorphic_path(name, v.span))
                }
                _ => (k.clone(), v.clone()),
            }
        })
        .collect();
    if !changed {
        return arg.clone();
    }
    Expr::new(
        arg.span,
        ExprNode::Hash {
            entries: new_entries,
            kwargs: true,
        },
    )
}

/// `RouteHelpers.<ivar_name>_path(@<ivar_name>.id)` — the explicit form
/// that replaces Rails' polymorphic `redirect_to @x`.
fn polymorphic_path(ivar_name: &Symbol, span: Span) -> Expr {
    let ivar_id = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar(ivar_name.as_str(), span)),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let helper_name = format!("{}_path", ivar_name.as_str());
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(const_path(&["RouteHelpers"], span)),
            method: Symbol::from(helper_name),
            args: vec![ivar_id],
            block: None,
            parenthesized: true,
        },
    )
}

// ---------------------------------------------------------------------------
// `<x>_path` / `<x>_url` route-helper prefix. Bare calls to route
// helpers (`Send` with no recv whose method ends in `_path` or `_url`)
// get the `RouteHelpers.` receiver added. Spinel's runtime defines
// every helper as a module function on `RouteHelpers`; controllers
// and tests must reach them through that namespace, since the
// `xxx_path` / `xxx_url` magic Rails injects via include doesn't
// exist here.
//
// This pass runs AFTER `rewrite_redirect_to` so the polymorphic
// rewrite's freshly-synthesized `RouteHelpers.x_path(...)` calls (which
// have a recv) are skipped — only original bare calls get the prefix.
//
// `shadowed` are names the caller's own scope DEFINES as methods — a
// controller's (or its ancestors') own `def post_authenticating_url`,
// `def logo_path(filename)`, `def next_page_url`. The suffix alone is a
// heuristic, and Ruby's answer is not a heuristic: a defined method wins
// over anything Rails injects. Without the set, campfire's
// `redirect_to post_authenticating_url` — a private method on the
// Authentication concern — became `RouteHelpers.post_authenticating_path`
// and raised NoMethodError on a module that has no such route. Mastodon
// has 120 controller methods with this shape.
// ---------------------------------------------------------------------------

/// The `Rails.application.routes.url_helpers` receiver chain — Rails'
/// spelling for reaching a path helper from somewhere with no helper
/// mixin (a model, a job, a test body).
fn is_rails_url_helpers_chain(e: &Expr) -> bool {
    let mut node = &*e.node;
    for step in ["url_helpers", "routes", "application"] {
        let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = node else {
            return false;
        };
        if method.as_str() != step || !args.is_empty() {
            return false;
        }
        node = &*r.node;
    }
    matches!(node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Rails")
}

/// Drop that receiver, leaving the BARE call the rewrite below already
/// knows how to ground.
///
/// A pre-pass rather than another arm in the map: the arm would have to
/// reproduce the id-projection the bare form gets, because `map_expr`
/// does not descend into a replacement it just made. Normalizing first
/// means the two spellings take exactly one path, which is the point —
/// campfire's `Webhook#message_path` and its own test write the same
/// call, and only the model's was grounded.
fn strip_url_helpers_receiver(expr: &Expr) -> Expr {
    map_expr(expr, &|e| {
        let ExprNode::Send { recv: Some(r), method, args, block, parenthesized } = &*e.node
        else {
            return None;
        };
        if !(method.as_str().ends_with("_path") || method.as_str().ends_with("_url")) {
            return None;
        }
        if !is_rails_url_helpers_chain(r) {
            return None;
        }
        Some(Expr::new(
            e.span,
            ExprNode::Send {
                recv: None,
                method: method.clone(),
                args: args.iter().map(strip_url_helpers_receiver).collect(),
                block: block.as_ref().map(strip_url_helpers_receiver),
                parenthesized: *parenthesized,
            },
        ))
    })
}

pub fn rewrite_route_helpers(
    expr: &Expr,
    shadowed: &HashSet<Symbol>,
    id_segments: &std::collections::HashMap<String, Vec<bool>>,
) -> Expr {
    let expr = &strip_url_helpers_receiver(expr);
    map_expr(expr, &|e| match &*e.node {
        ExprNode::Send { recv: None, method, args, block, parenthesized }
            if (method.as_str().ends_with("_path")
                || method.as_str().ends_with("_url"))
                && !shadowed.contains(method) =>
        {
            // `RouteHelpers` only emits `_path` helpers — Rails'
            // `_url` form differs by host prefix, which we don't
            // model. Fold `_url` onto its `_path` twin so test/
            // controller bodies that use the URL form resolve.
            let raw = method.as_str();
            let dispatch_method = if let Some(stem) = raw.strip_suffix("_url") {
                Symbol::from(format!("{stem}_path"))
            } else {
                method.clone()
            };
            // Polymorphic AR-instance → `.id` extraction. Rails
            // accepts `article_url(@article)` and dispatches via
            // implicit `.id`; the route_helpers in this codebase take
            // an `id: number` directly. Extract `.id` for:
            //   - Ivar args (`@article` → `@article.id`)
            //   - Class-method calls on capital-named Const recvs
            //     (`Article.last`, `Article.find(1)` → `<x>.id`).
            //     Heuristic at lower time: a Send whose recv is a
            //     Const with capitalized first segment is almost
            //     always a model class method returning an instance.
            // Already-projected args (`@article.id`) pass through
            // since they're Sends with method `id` — adding another
            // `.id` would double-wrap, so detect that shape.
            // …and only where the SEGMENT this argument fills is
            // id-shaped. campfire's `join_url(@join_code)` fills
            // `:join_code`, a string column, and the blind projection
            // emitted `@join_code.id` — `undefined method 'id' for an
            // instance of String`, on the page every join link points
            // at. A helper this table does not know keeps the old
            // blind behaviour: the table is built from the app's own
            // routes, so a miss means the call is not a route helper
            // and the shape test below is the only signal there is.
            let shape = id_segments.get(dispatch_method.as_str());
            let projected_args: Vec<Expr> = args
                .iter()
                .enumerate()
                .map(|(i, arg)| {
                    let id_segment =
                        shape.map_or(true, |s| s.get(i).copied().unwrap_or(true));
                    let needs_id = id_segment && match &*arg.node {
                        ExprNode::Ivar { .. } => true,
                        ExprNode::Send { recv: Some(r), method, .. }
                            if method.as_str() != "id" =>
                        {
                            matches!(
                                &*r.node,
                                ExprNode::Const { path }
                                    if path.first().map(|s| {
                                        s.as_str()
                                            .chars()
                                            .next()
                                            .is_some_and(|c| c.is_ascii_uppercase())
                                    }).unwrap_or(false)
                            )
                        }
                        _ => false,
                    };
                    if needs_id {
                        Expr::new(
                            arg.span,
                            ExprNode::Send {
                                recv: Some(arg.clone()),
                                method: Symbol::from("id"),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        )
                    } else {
                        rewrite_route_helpers(arg, shadowed, id_segments)
                    }
                })
                .collect();
            Some(Expr::new(
                e.span,
                ExprNode::Send {
                    recv: Some(const_path(&["RouteHelpers"], e.span)),
                    method: dispatch_method,
                    args: projected_args,
                    block: block
                        .as_ref()
                        .map(|b| rewrite_route_helpers(b, shadowed, id_segments)),
                    parenthesized: *parenthesized,
                },
            ))
        }
        _ => None,
    })
}

/// A RECORD reaching a route helper projects to its id.
///
/// `rewrite_route_helpers` above already does this, shape-directed, at
/// the moment it adds the `RouteHelpers.` receiver. But shape is not
/// enough in a test body: at that moment `room_path(rooms(:watercooler))`
/// still holds a bare fixture call (no receiver at all), and
/// `room_path(users(:david).rooms.original)` holds a chain whose head is
/// not a Const either. Both are records, and both reached the helper
/// whole — campfire's rooms tests asserted a redirect to
/// `/rooms/#<Room:0x000000012339eda0>`.
///
/// So this pass asks the TYPE instead, which means it must run after the
/// body-typer. A route helper's segment parameter is an id; an argument
/// that types to a class is a record standing where its id belongs.
/// Idempotent — a projected argument types `Integer`, not a class.
pub fn project_route_helper_ids(expr: &Expr) -> Expr {
    map_expr(expr, &|e| {
        let ExprNode::Send { recv: Some(r), method, args, block, parenthesized } = &*e.node else {
            return None;
        };
        if !matches!(&*r.node, ExprNode::Const { path }
            if path.len() == 1 && path[0].as_str() == "RouteHelpers")
        {
            return None;
        }
        if !(method.as_str().ends_with("_path") || method.as_str().ends_with("_url")) {
            return None;
        }
        if !args.iter().any(arg_carries_a_model) {
            return None;
        }
        let projected: Vec<Expr> = args.iter().map(project_arg).collect();
        Some(Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(r.clone()),
                method: method.clone(),
                args: projected,
                block: block.clone(),
                parenthesized: *parenthesized,
            },
        ))
    })
}

/// A route-helper argument with every model instance in it projected
/// to its id — the argument itself, or the VALUES of a trailing kwargs
/// hash.
///
/// The hash is Rails' query-string half: `room_messages_url(@room,
/// before: @messages.third)` renders `?before=<param>`, and Rails puts
/// each value through `to_param`, which on a record is its id. The
/// generated helper calls `.to_s`, so a record reaching one rendered
/// `?before=%23%3CMessage%3A0x...%3E` and the router matched no route
/// at all — campfire's `messages_controller_test` pages this way twice.
///
/// Only the VALUES move. A key is a Symbol by construction.
fn project_arg(a: &Expr) -> Expr {
    if let ExprNode::Hash { entries, kwargs } = &*a.node {
        if entries.iter().any(|(_, v)| records_a_model(v)) {
            let projected = entries
                .iter()
                .map(|(k, v)| (k.clone(), if records_a_model(v) { id_of(v) } else { v.clone() }))
                .collect();
            return Expr::new(a.span, ExprNode::Hash { entries: projected, kwargs: *kwargs });
        }
        return a.clone();
    }
    if records_a_model(a) {
        return id_of(a);
    }
    a.clone()
}

/// Is there a model instance anywhere `project_arg` would reach?
fn arg_carries_a_model(a: &Expr) -> bool {
    match &*a.node {
        ExprNode::Hash { entries, .. } => entries.iter().any(|(_, v)| records_a_model(v)),
        _ => records_a_model(a),
    }
}

fn id_of(a: &Expr) -> Expr {
    Expr::new(
        a.span,
        ExprNode::Send {
            recv: Some(a.clone()),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// Does this argument carry a model instance? Only a positively
/// class-typed expression counts — an unknown type stays untouched
/// rather than guessing, which is the half `rewrite_route_helpers`'
/// shape heuristic already covers for the ivar and `Model.find(…)`
/// forms it can see.
fn records_a_model(arg: &Expr) -> bool {
    fn is_record(t: &crate::ty::Ty) -> bool {
        match t {
            crate::ty::Ty::Class { .. } => true,
            // `user.rooms.last` types `Room | Nil` — the catalog's
            // SelfOrNil. Still a record standing where an id belongs;
            // Rails would fail on the nil too.
            crate::ty::Ty::Union { variants } => {
                variants.iter().any(|v| matches!(v, crate::ty::Ty::Class { .. }))
                    && variants
                        .iter()
                        .all(|v| matches!(v, crate::ty::Ty::Class { .. } | crate::ty::Ty::Nil))
            }
            _ => false,
        }
    }
    arg.ty.as_ref().is_some_and(is_record)
}

fn const_path(segments: &[&str], span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Const {
            path: segments.iter().map(|s| Symbol::from(*s)).collect(),
        },
    )
}

/// True when `e` is a bare `params` send: no receiver, no args, no
/// block. This is the recv shape `params.expect(...)` parses to.
fn is_bare_params(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "params" && args.is_empty()
    )
}

/// Lower `params.expect(...)` based on its argument shape:
/// - `params.expect(:id)` → `@params[:id].to_i`
/// - `params.expect(post: [:title, :body])` → `@params.require(:post).permit(:title, :body)`
///
/// Anything else (no args, multi-arg, unrecognized arg shape) is left
/// as `@params.expect(args...)` so we don't silently drop an idiom we
/// don't yet understand. The lowerer's job is rewrite, not erasure.
fn rewrite_expect(
    args: &[Expr],
    block: Option<&Expr>,
    parenthesized: bool,
    span: Span,
) -> Expr {
    if args.len() == 1 {
        let arg = &args[0];
        // Single-symbol form → @params[:sym].to_i
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node {
            return params_index_to_i(value, span);
        }
        // Single-keyword-hash form → @params.require(:k).permit(:f1, :f2, ...)
        if let ExprNode::Hash { entries, .. } = &*arg.node {
            if let Some(pair) = single_resource_hash(entries) {
                return params_require_permit(pair.0, pair.1, span);
            }
        }
    }
    // Fallback: keep .expect with @params recv. Rewrite the args
    // recursively so any nested `params` references inside them get
    // lowered too.
    let recv = ivar("params", span);
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("expect"),
            args: args.iter().map(rewrite_params).collect(),
            block: block.map(rewrite_params),
            parenthesized,
        },
    )
}

/// `@params.fetch("sym", "0").to_i` — used for the single-symbol
/// expect shape. `fetch` with a default returns non-nil so the
/// `.to_i` chain compiles under strict targets (Crystal). Default
/// `"0"` parses to integer 0 — matches the spinel-blog convention
/// for missing-id-as-unsaved-sentinel. String key matches the
/// request-body parser's String-keyed Hash; a Symbol key would miss.
fn params_index_to_i(sym: &Symbol, span: Span) -> Expr {
    // `@params.fetch("<sym>", "0").to_s.to_i` — the leading `.to_s`
    // bridges the recursive `Roundhouse::ParamValue` union (String |
    // Hash | Array) into a single String, so the subsequent `.to_i`
    // type-checks on strict targets. For the only access pattern
    // this rewrite covers (`params.expect(:id)` scalar lookup), the
    // value is always a String leaf at runtime — the `.to_s` is a
    // no-op on String (Ruby/Crystal) / `String(x)` coercion (TS),
    // matching Rails' string-default param semantics.
    let fetched = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar("params", span)),
            method: Symbol::from("fetch"),
            args: vec![
                Expr::new(
                    span,
                    ExprNode::Lit {
                        value: Literal::Str { value: sym.as_str().to_string() },
                    },
                ),
                Expr::new(
                    span,
                    ExprNode::Lit { value: Literal::Str { value: "0".to_string() } },
                ),
            ],
            block: None,
            parenthesized: true,
        },
    );
    let to_s = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(fetched),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(to_s),
            method: Symbol::from("to_i"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// `@params.require(:resource).permit(:f1, :f2, ...)` — the strong-
/// params chain spinel's runtime implements. Returns None at the call
/// site if the entries don't match the single-resource shape.
fn single_resource_hash(entries: &[(Expr, Expr)]) -> Option<(Symbol, Vec<Symbol>)> {
    if entries.len() != 1 {
        return None;
    }
    let (k, v) = &entries[0];
    let resource = match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.clone(),
        _ => return None,
    };
    let fields = match &*v.node {
        ExprNode::Array { elements, .. } => {
            let mut out = Vec::with_capacity(elements.len());
            for el in elements {
                match &*el.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => out.push(value.clone()),
                    _ => return None,
                }
            }
            out
        }
        _ => return None,
    };
    Some((resource, fields))
}

fn params_require_permit(resource: Symbol, fields: Vec<Symbol>, span: Span) -> Expr {
    let require_sym = Expr::new(
        span,
        ExprNode::Lit { value: Literal::Sym { value: resource } },
    );
    let require_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar("params", span)),
            method: Symbol::from("require"),
            args: vec![require_sym],
            block: None,
            parenthesized: true,
        },
    );
    // Emit `permit([:f1, :f2, ...])` — single Array arg, not splat.
    // Monomorphic parameter slot for spinel + type-strict targets;
    // every per-target Parameters runtime takes Array[Symbol] here.
    let permit_array_elems: Vec<Expr> = fields
        .into_iter()
        .map(|f| Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: f } }))
        .collect();
    let permit_array = Expr::new(
        span,
        ExprNode::Array {
            elements: permit_array_elems,
            style: ArrayStyle::Brackets,
        },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(require_call),
            method: Symbol::from("permit"),
            args: vec![permit_array],
            block: None,
            parenthesized: true,
        },
    )
}

fn nil_expr(span: Span) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Nil })
}

fn ivar(name: &str, span: Span) -> Expr {
    Expr::new(span, ExprNode::Ivar { name: Symbol::from(name) })
}

/// `@flash[:<key>]` — used by render-rewrite to pass the controller's
/// flash slots through to view extra_params.
fn flash_lookup(span: Span, key: &str) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(ivar("flash", span)),
            method: Symbol::from("[]"),
            args: vec![Expr::new(
                span,
                ExprNode::Lit {
                    value: Literal::Sym { value: Symbol::from(key) },
                },
            )],
            block: None,
            parenthesized: false,
        },
    )
}
