//! Rails controller ingestion — parses one `app/controllers/*.rb`
//! into a `Controller`, splitting class-body items into actions,
//! filters, a `private` marker, and unknown fall-throughs.

use ruby_prism::Node;

use crate::dialect::{Action, Comment, Controller, ControllerBodyItem, LayoutDecl, RenderTarget};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::span::Span;
use crate::ty::{Row, Ty};
use crate::{ClassId, Symbol};

use super::expr::ingest_expr;
use super::util::{
    class_name_path, collect_comments, constant_id_str, constant_path_of, drain_comments_before,
    find_all_classes_with_scope, find_first_class, flatten_statements, source_has_blank_line,
    symbol_list_style, symbol_list_value, symbol_value,
};
use super::{IngestError, IngestResult};

pub fn ingest_controller(source: &[u8], file: &str) -> IngestResult<Option<Controller>> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    // A controller file may declare several top-level classes — e.g.
    // `login_controller.rb` defines `LoginBannedError < StandardError`
    // (and siblings) *before* `class LoginController`. The controller
    // is the class whose name ends in `Controller`, not the first
    // class in the file; picking the first ingests an empty error
    // class as the controller and drops every real action (so its
    // view ivars never resolve). Fall back to the first class when no
    // name matches the convention.
    let all_classes = find_all_classes_with_scope(&root);
    let chosen_idx = all_classes.iter().position(|(_, c)| {
        class_name_path(c)
            .and_then(|p| p.last().cloned())
            .is_some_and(|last| last.ends_with("Controller"))
    });
    // Empty-bodied top-level siblings (`class LoginFailedError <
    // StandardError; end` ahead of the controller class) are pure
    // declarations the actions raise/rescue — carry them as (name,
    // parent) pairs so emit can re-declare them. Captured only when a
    // conventionally-named controller class was found (the fallback
    // path below has no principled sibling/controller split), and only
    // the empty-body + explicit-superclass shape; anything richer
    // stays dropped as before.
    let mut sibling_classes: Vec<(Symbol, Symbol)> = Vec::new();
    if chosen_idx.is_some() {
        for (i, (scope, c)) in all_classes.iter().enumerate() {
            if Some(i) == chosen_idx || !scope.is_empty() {
                continue;
            }
            let body_empty = c
                .body()
                .is_none_or(|b| flatten_statements(b).is_empty());
            if !body_empty {
                continue;
            }
            let Some(path) = class_name_path(c) else { continue };
            let Some(parent_path) =
                c.superclass().and_then(|n| constant_path_of(&n))
            else {
                continue;
            };
            sibling_classes.push((
                Symbol::from(path.join("::")),
                Symbol::from(parent_path.join("::")),
            ));
        }
    }
    let class = match chosen_idx {
        Some(i) => all_classes.into_iter().nth(i).map(|(_, c)| c),
        None => find_first_class(&root),
    };
    let Some(class) = class else {
        return Ok(None);
    };

    let name_path = class_name_path(&class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "controller class name must be a simple constant or path".into(),
    })?;

    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });

    let mut comments = collect_comments(&result);
    drain_comments_before(&mut comments, class.location().start_offset());
    let mut body_items: Vec<ControllerBodyItem> = Vec::new();
    let mut layout = LayoutDecl::Inherit;
    if let Some(class_body) = class.body() {
        let mut prev_end: Option<usize> = None;
        for stmt in flatten_statements(class_body) {
            let stmt_start = stmt.location().start_offset();
            let leading_area_start =
                comments.first().map(|(off, _)| *off).filter(|off| *off < stmt_start)
                    .unwrap_or(stmt_start);
            let mut leading = drain_comments_before(&mut comments, stmt_start);
            let leading_blank = prev_end
                .map(|pe| source_has_blank_line(source, pe, leading_area_start))
                .unwrap_or(false);
            // Recognize `layout :name` / `layout "name"` / `layout false`
            // at the controller class level. Last declaration wins
            // (matches Rails: a later `layout` call overrides an earlier
            // one). The call still falls through to Unknown for source
            // round-trip; the side-channel `Controller.layout` is what
            // analyze reads to seed layout-view ivar types.
            if let Some(decl) = parse_layout_call(&stmt) {
                layout = decl;
            }
            // A `before_action :a, :b` line declares one filter per leading
            // symbol; expand to one `Filter` body item each so every target's
            // ivar assignments seed the actions it guards (the single-target
            // parse only ever captured the first symbol). Block-form filters
            // (`before_action { ... }`, no symbol target) return `None` here,
            // fall through to `Unknown`, and round-trip verbatim — analyze
            // harvests their ivars separately.
            if let Some(filters) = parse_filter_call(&stmt) {
                for (i, filter) in filters.into_iter().enumerate() {
                    body_items.push(ControllerBodyItem::Filter {
                        filter,
                        leading_comments: if i == 0 {
                            std::mem::take(&mut leading)
                        } else {
                            Vec::new()
                        },
                        leading_blank_line: i == 0 && leading_blank,
                    });
                }
                prev_end = Some(stmt.location().end_offset());
                continue;
            }
            // Survey mode: an unsupported item costs itself, not the whole
            // controller — record the gap and keep walking (same gate as
            // the model walk; see ingest/model.rs). Strict mode aborts.
            let mut item = match ingest_controller_body_item(&stmt, file, leading) {
                Ok(item) => item,
                Err(err) if super::survey::is_active() => {
                    super::survey::record(&err);
                    prev_end = Some(stmt.location().end_offset());
                    continue;
                }
                Err(err) => return Err(err),
            };
            item.set_leading_blank_line(leading_blank);
            body_items.push(item);
            prev_end = Some(stmt.location().end_offset());
        }
    }

    Ok(Some(Controller {
        name: ClassId(Symbol::from(name_path.join("::"))),
        parent,
        body: body_items,
        layout,
        sibling_classes,
    }))
}

/// Recognize a `layout` class-body call. Returns `Some(decl)` if this
/// is a `layout ...` call we can interpret, `None` otherwise (including
/// for unsupported shapes like `layout :method_name` where the symbol
/// names a controller method — those degrade to `Inherit` and the
/// effective layout falls back to convention).
///
/// Note: we can't tell `layout :foo` (static name "foo") from
/// `layout :foo` (dispatch to method `foo`) syntactically. v1 treats
/// every `layout :sym` as a static name. The dispatch form is rare
/// enough on real Rails controllers that this is a safe v1 assumption;
/// the worst case is a layout-view-name miss, not a crash.
fn parse_layout_call(stmt: &Node<'_>) -> Option<LayoutDecl> {
    let call = stmt.as_call_node()?;
    if call.receiver().is_some() {
        return None;
    }
    if constant_id_str(&call.name()) != "layout" {
        return None;
    }
    let args = call.arguments()?;
    let all_args = args.arguments();
    let first = all_args.iter().next()?;
    if let Some(sym) = first.as_symbol_node() {
        let bytes = sym.unescaped();
        let name = std::str::from_utf8(bytes).ok()?;
        return Some(LayoutDecl::Name { name: Symbol::from(name) });
    }
    if let Some(s) = first.as_string_node() {
        let bytes = s.unescaped();
        let name = std::str::from_utf8(bytes).ok()?;
        return Some(LayoutDecl::Name { name: Symbol::from(name) });
    }
    if first.as_false_node().is_some() || first.as_nil_node().is_some() {
        return Some(LayoutDecl::None);
    }
    None
}

/// Classify one class-body statement into its `ControllerBodyItem` variant.
fn ingest_controller_body_item(
    stmt: &Node<'_>,
    file: &str,
    leading_comments: Vec<Comment>,
) -> IngestResult<ControllerBodyItem> {
    if let Some(def) = stmt.as_def_node() {
        let action_name = constant_id_str(&def.name()).to_string();
        let body_expr = match def.body() {
            Some(b) => ingest_expr(&b, file)?,
            None => {
                // Empty `def show; end` — no body node to take a span
                // from; use the def's own span so downstream synthesis
                // (implicit render, format dispatch) attributes to the
                // action declaration rather than rendering location-less.
                let loc = def.location();
                let span = Span {
                    file: super::sources::file_id(file),
                    start: loc.start_offset() as u32,
                    end: loc.end_offset() as u32,
                };
                Expr::new(span, ExprNode::Seq { exprs: vec![] })
            }
        };
        let renders = infer_render_template(&body_expr)
            .map(|name| RenderTarget::Template { name, formats: Vec::new() })
            .unwrap_or(RenderTarget::Inferred);
        // Capture required positional params (`def period(query)`).
        // Routed actions take none (they read `params[...]`); helper
        // methods do, and without the param names the analyzer can't
        // seed their types from call sites — so a body like
        // `query.where(...)` never resolves and the method's return
        // type is lost. Value types are placeholders (`Untyped`); the
        // real types come from the inferred-params table. Optional /
        // keyword / rest params need richer modeling and stay
        // unhandled for now.
        let mut params = Row::closed();
        let mut opt_params: Vec<(Symbol, Expr)> = Vec::new();
        let mut block_param: Option<Symbol> = None;
        if let Some(pn) = def.parameters() {
            for req in pn.requireds().iter() {
                if let Some(rp) = req.as_required_parameter_node() {
                    params
                        .fields
                        .insert(Symbol::from(constant_id_str(&rp.name())), Ty::Untyped);
                }
            }
            // Optional positionals (`opts = {}`) — keep the name + default
            // so the emitted signature round-trips. Keyword / rest / post
            // params still need richer modeling and stay unhandled.
            for opt in pn.optionals().iter() {
                if let Some(op) = opt.as_optional_parameter_node() {
                    let name = Symbol::from(constant_id_str(&op.name()));
                    let default = ingest_expr(&op.value(), file)?;
                    opt_params.push((name, default));
                }
            }
            // Block param (`&block`) — methods that name their block so
            // the body can pass it on (`&block`) or that crash without the
            // arity slot.
            if let Some(bn) = pn.block() {
                if let Some(n) = bn.name() {
                    block_param = Some(Symbol::from(constant_id_str(&n)));
                }
            }
        }
        return Ok(ControllerBodyItem::Action {
            action: Action {
                name: Symbol::from(action_name),
                params,
                opt_params,
                block_param,
                body: body_expr,
                renders,
                effects: EffectSet::pure(),
            },
            leading_comments,
            leading_blank_line: false,
        });
    }
    if let Some(call) = stmt.as_call_node() {
        if call.receiver().is_some() {
            return Ok(ControllerBodyItem::Unknown {
                expr: ingest_expr(stmt, file)?,
                leading_comments,
                leading_blank_line: false,
            });
        }
        let method = constant_id_str(&call.name()).to_string();
        // Filter calls (`before_action` etc.) are intercepted by the caller,
        // which expands multi-symbol forms into one `Filter` item per target.
        if method == "private" && call.arguments().is_none() && call.block().is_none() {
            return Ok(ControllerBodyItem::PrivateMarker {
                leading_blank_line: false,
                leading_comments,
            });
        }
        return Ok(ControllerBodyItem::Unknown {
            expr: ingest_expr(stmt, file)?,
            leading_comments,
            leading_blank_line: false,
        });
    }
    Ok(ControllerBodyItem::Unknown {
        expr: ingest_expr(stmt, file)?,
        leading_comments,
        leading_blank_line: false,
    })
}

/// Recognize a controller filter declaration (`before_action`,
/// `around_action`, `after_action`, `skip_before_action`) and return one
/// [`Filter`] per leading symbol target, all sharing the call's `only:` /
/// `except:` scoping. Rails runs every named target on the same actions,
/// so `before_action :a, :b, only: [:x]` becomes two filters guarding `x`
/// — the previous single-target parse silently dropped every symbol after
/// the first, hiding their ivar assignments from analyze and their calls
/// from the emitted dispatch chain.
///
/// Returns `None` for non-filter calls and for filter calls with no symbol
/// target — notably the block form `before_action { ... }`, which has no
/// named method to reference. Those fall through to `Unknown`, round-trip
/// verbatim, and have their ivars harvested directly during analyze.
fn parse_filter_call(stmt: &Node<'_>) -> Option<Vec<crate::dialect::Filter>> {
    use crate::dialect::{Filter, FilterKind};

    let call = stmt.as_call_node()?;
    if call.receiver().is_some() {
        return None;
    }
    let kind = match constant_id_str(&call.name()) {
        "before_action" => FilterKind::Before,
        "around_action" => FilterKind::Around,
        "after_action" => FilterKind::After,
        "skip_before_action" => FilterKind::Skip,
        _ => return None,
    };

    let args = call.arguments()?;

    let mut targets: Vec<Symbol> = Vec::new();
    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    let mut only_style = crate::expr::ArrayStyle::default();
    let mut except_style = crate::expr::ArrayStyle::default();

    for arg in args.arguments().iter() {
        if let Some(sym) = symbol_value(&arg) {
            targets.push(Symbol::from(sym.as_str()));
            continue;
        }
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = assoc.value();
            match key.as_str() {
                "only" => {
                    only = symbol_list_value(&value);
                    only_style = symbol_list_style(&value);
                }
                "except" => {
                    except = symbol_list_value(&value);
                    except_style = symbol_list_style(&value);
                }
                _ => {}
            }
        }
    }

    if targets.is_empty() {
        return None;
    }

    Some(
        targets
            .into_iter()
            .map(|target| Filter {
                kind: kind.clone(),
                target,
                only: only.clone(),
                except: except.clone(),
                only_style,
                except_style,
            })
            .collect(),
    )
}

/// Resolve the template an action explicitly renders, so the analyzer can
/// bind its ivars to the view it actually produces rather than the
/// convention `controller/<action_name>`. Returns `Some(name)` only when
/// the body renders exactly one distinct template *outside* any
/// `respond_to` block.
///
/// The `respond_to do |format| … end` exclusion is the safety property:
/// format dispatch routinely renders several templates by MIME type
/// (`format.json { render :show }` next to `format.html { render :new }`),
/// so no single one is "the action's view" — those actions keep their
/// convention name. A plain top-level `render :show` (the "reuse another
/// action's template" idiom — e.g. RepliesController's
/// all/comments/stories/unread all `render :show`) is unambiguous, and is
/// exactly the shape whose view otherwise received no controller ivars.
fn infer_render_template(body: &Expr) -> Option<Symbol> {
    let mut names: Vec<Symbol> = Vec::new();
    collect_template_renders(body, &mut names);
    let first = names.first()?.clone();
    // Conflicting templates (`if … render :a else render :b`) are
    // ambiguous — fall back to the convention name.
    names.iter().all(|n| *n == first).then_some(first)
}

/// Walk an action body collecting every explicit template-render name,
/// skipping `respond_to` blocks (their renders are format-specific).
fn collect_template_renders(expr: &Expr, out: &mut Vec<Symbol>) {
    if let ExprNode::Send { recv, method, args, .. } = &*expr.node {
        if recv.is_none() {
            match method.as_str() {
                "respond_to" => return,
                "render" => {
                    if let Some(name) = render_template_name(args) {
                        out.push(name);
                    }
                }
                _ => {}
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_template_renders(c, out));
}

/// Extract a template name from `render` arguments for the forms that name
/// a template view: `render :show`, `render "users/show"`,
/// `render template: "x"`, `render action: :y`. Returns `None` for the
/// non-template forms (`render json:/plain:/partial:/inline:/…`,
/// `render @record`, bare `render`) so they keep convention semantics.
fn render_template_name(args: &[Expr]) -> Option<Symbol> {
    let first = args.first()?;
    match &*first.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(Symbol::from(value.as_str())),
        // `render template: "x"` / `render action: :y` — a leading options
        // hash. Only `template:`/`action:` name a view; every other key
        // (`json:`, `plain:`, `partial:`, `status:`, …) does not.
        ExprNode::Hash { entries, .. } => entries.iter().find_map(|(k, v)| {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                return None;
            };
            if !matches!(key.as_str(), "template" | "action") {
                return None;
            }
            match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                ExprNode::Lit { value: Literal::Str { value } } => {
                    Some(Symbol::from(value.as_str()))
                }
                _ => None,
            }
        }),
        _ => None,
    }
}
