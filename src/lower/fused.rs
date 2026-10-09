//! One post-order walk for independent post-analyze send rewrites.
//!
//! Each of these passes used to call [`super::for_each_hook_body`] (and
//! usually walk views too) on its own. The rewrites do not consume each
//! other's output — they match disjoint method names — so applying them
//! at every node in one walk is equivalent to N sequential walks and
//! drops the quadratic tree traffic on large apps.
//!
//! Surfaces stay the same as the original per-pass walks: every rewrite
//! runs on hook bodies; view/test coverage matches the pass it came from.
//! Most fused rewrites match disjoint method names. The mid-group
//! `route_format_suffix` wrap (`call + ".ext"`) is the exception:
//! position/host follow-ups still run on that inner helper.
//!
//! Six walks, split by ordering constraints: the early group runs
//! before any pass that mutates argument lists; the context group sits
//! after mocha (`try_guard` last, so minted method names are not fed
//! back into this walk); the pre-tag group sits after the STI
//! predecessor cluster and before `tag_builder`; the narrow and mid
//! groups sit after `tag_builder` and before `kwsplat`; the late group
//! runs after `kwsplat` (and after `tag_builder`, which `capture_inline`
//! depends on).

use crate::app::App;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use std::collections::{BTreeSet, HashSet};

pub fn apply_fused_independent_rewrites(app: &mut App) {
    let skip_exclude = super::exclude_predicate::app_defines_exclude(app);
    let skip_in = super::in_predicate::app_defines_in(app);
    let skip_including = super::including::app_defines_including(app);
    let skip_full_messages = super::errors_full_messages::app_defines_full_messages(app);

    super::for_each_hook_body(app, &mut |body| {
        walk_postorder(body, &mut |e| {
            rewrite_hook_node(e, skip_exclude, skip_in, skip_including, skip_full_messages);
        });
    });
    for view in &mut app.views {
        walk_postorder(&mut view.body, &mut |e| {
            rewrite_view_node(e, skip_exclude, skip_in, skip_including, skip_full_messages);
        });
    }
    super::for_each_test_body(app, &mut |body| {
        walk_postorder(body, &mut |e| rewrite_test_node(e, skip_full_messages));
    });
}

/// Independent rewrites that must observe ingested (or already
/// `kwsplat`-expanded) argument lists, and `capture_inline` which
/// must see `tag_builder`'s synthesized `capture` blocks.
pub fn apply_fused_late_rewrites(app: &mut App) {
    super::for_each_hook_body(app, &mut |body| {
        walk_postorder(body, &mut rewrite_late_hook_node);
    });
    for view in &mut app.views {
        walk_postorder(&mut view.body, &mut super::attachables_grep::rewrite_node);
    }
    super::for_each_test_body(app, &mut |body| {
        walk_postorder(body, &mut super::attachables_grep::rewrite_node);
    });
}

/// Context-heavy independent send rewrites that still do a full tree
/// walk each: collect per-pass tables once, then one hook walk, one
/// test walk, and one view walk matching the original surfaces.
/// `try_guard` is last: it mints arbitrary method names from a literal
/// symbol, so a later fused rewrite that keyed on those names would see
/// new children the sequential order never walked. `attribute_aliases`
/// still runs after this walk for that reason.
pub fn apply_fused_context_rewrites(app: &mut App) {
    let formats = std::mem::take(&mut app.time_formats);
    let mut gid_models: BTreeSet<Symbol> = BTreeSet::new();
    let mut gid_signed: BTreeSet<Symbol> = BTreeSet::new();
    let materialized = super::assoc_pluck::materialized_assoc_names(app);
    let try_definers = super::try_guard::collect_definers(app);
    let try_parents = super::try_guard::collect_parents(app);

    super::for_each_hook_body(app, &mut |body| {
        walk_postorder(body, &mut |e| {
            super::time_current::rewrite_node(e, &formats);
            super::global_id_locate::rewrite_node(e, &mut gid_models, &mut gid_signed);
            super::assoc_pluck::rewrite_node(e, &materialized);
            super::try_guard::rewrite_node(e, &try_definers, &try_parents);
        });
    });
    super::for_each_test_body(app, &mut |body| {
        walk_postorder(body, &mut |e| {
            super::time_current::rewrite_node(e, &formats);
            super::webmock::rewrite_node(e);
            super::try_guard::rewrite_node(e, &try_definers, &try_parents);
        });
    });
    // `try_guard` also rewrites test constants and inner-class methods,
    // which `for_each_test_body` does not reach. The other context
    // rewrites never walked those surfaces.
    for tm in &mut app.test_modules {
        for (_, value) in &mut tm.constants {
            walk_postorder(value, &mut |e| {
                super::try_guard::rewrite_node(e, &try_definers, &try_parents);
            });
        }
        for ic in &mut tm.inner_classes {
            for m in &mut ic.methods {
                walk_postorder(&mut m.body, &mut |e| {
                    super::try_guard::rewrite_node(e, &try_definers, &try_parents);
                });
            }
        }
    }
    for view in &mut app.views {
        walk_postorder(&mut view.body, &mut |e| {
            super::global_id_locate::rewrite_node(e, &mut gid_models, &mut gid_signed);
            super::try_guard::rewrite_node(e, &try_definers, &try_parents);
        });
    }

    app.time_formats = formats;
    app.global_id_locate_models.extend(gid_models);
    app.global_id_locate_signed_models.extend(gid_signed);
}

/// Independent rewrites after the STI predecessor cluster and before
/// `tag_builder`. Surfaces match the original per-pass walks:
/// `attribute_aliases` starts with an in-model walk then hooks/tests/views;
/// `sti_is_a` is hooks+views; job/cookie tests are tests only;
/// `controller_class_render` is hooks plus per-test attachment locals;
/// `sum_symbol` is model methods/unknown last so its new `to_a`/`sum`
/// children are not fed to the other fused rewrites.
pub fn apply_fused_pre_tag_rewrites(app: &mut App) {
    let models: HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .collect();
    let sti_names = super::sti_is_a::subclass_type_names(app);
    let contracts = super::controller_class_render::call_contracts(app);
    let none = HashSet::new();

    for model in &mut app.models {
        for item in &mut model.body {
            use crate::dialect::{Association, ModelBodyItem};
            match item {
                ModelBodyItem::Method { method, .. } => {
                    walk_postorder(&mut method.body, &mut |e| {
                        super::attribute_aliases::rewrite_node(e, &models, true);
                    });
                }
                ModelBodyItem::Scope { scope, .. } => {
                    walk_postorder(&mut scope.body, &mut |e| {
                        super::attribute_aliases::rewrite_node(e, &models, true);
                    });
                }
                ModelBodyItem::Callback { callback, .. } => {
                    if let Some(cond) = &mut callback.condition {
                        walk_postorder(cond, &mut |e| {
                            super::attribute_aliases::rewrite_node(e, &models, true);
                        });
                    }
                }
                ModelBodyItem::Unknown { expr, .. } => {
                    walk_postorder(expr, &mut |e| {
                        super::attribute_aliases::rewrite_node(e, &models, true);
                    });
                }
                ModelBodyItem::Association {
                    assoc: Association::HasMany { extension, .. },
                    ..
                } => {
                    for m in extension.iter_mut() {
                        walk_postorder(&mut m.body, &mut |e| {
                            super::attribute_aliases::rewrite_node(e, &models, true);
                        });
                    }
                }
                _ => {}
            }
        }
    }

    super::for_each_hook_body(app, &mut |body| {
        walk_postorder(body, &mut |e| {
            super::attribute_aliases::rewrite_node(e, &models, false);
            super::sti_is_a::rewrite_node(e, &sti_names);
            super::controller_class_render::rewrite_node(e, &contracts, &none);
        });
    });
    for view in &mut app.views {
        walk_postorder(&mut view.body, &mut |e| {
            super::attribute_aliases::rewrite_node(e, &models, false);
            super::sti_is_a::rewrite_node(e, &sti_names);
        });
    }
    for tm in &mut app.test_modules {
        let builders = super::controller_class_render::attachment_builders(&tm.helpers);
        if let Some(setup) = &mut tm.setup {
            let locals = super::controller_class_render::attachment_locals(setup, &builders);
            walk_postorder(setup, &mut |e| {
                super::attribute_aliases::rewrite_node(e, &models, false);
                super::job_test_only::rewrite_node(e);
                super::test_cookie_jar::rewrite_node(e);
                super::controller_class_render::rewrite_node(e, &contracts, &locals);
            });
        }
        for t in &mut tm.tests {
            let locals = super::controller_class_render::attachment_locals(&t.body, &builders);
            walk_postorder(&mut t.body, &mut |e| {
                super::attribute_aliases::rewrite_node(e, &models, false);
                super::job_test_only::rewrite_node(e);
                super::test_cookie_jar::rewrite_node(e);
                super::controller_class_render::rewrite_node(e, &contracts, &locals);
            });
        }
        for h in &mut tm.helpers {
            let locals = super::controller_class_render::attachment_locals(&h.body, &builders);
            walk_postorder(&mut h.body, &mut |e| {
                super::attribute_aliases::rewrite_node(e, &models, false);
                super::job_test_only::rewrite_node(e);
                super::test_cookie_jar::rewrite_node(e);
                super::controller_class_render::rewrite_node(e, &contracts, &locals);
            });
        }
    }

    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => {
                    walk_postorder(&mut method.body, &mut super::sum_symbol::rewrite_node);
                }
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => {
                    walk_postorder(expr, &mut super::sum_symbol::rewrite_node);
                }
                _ => {}
            }
        }
    }
}

/// Controller/library/model-only independent rewrites. Surfaces match
/// the original per-pass walks: `request_index` is controllers only;
/// `session_options`/`status_literal` add library methods;
/// `to_param_residue` adds model methods. Not folded into the mid hook
/// walk, which would rewrite `request[]` in helpers and views.
pub fn apply_fused_narrow_rewrites(app: &mut App) {
    use crate::dialect::{ControllerBodyItem, ModelBodyItem};
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            match item {
                ControllerBodyItem::Action { action, .. } => {
                    for (_name, default) in &mut action.opt_params {
                        walk_postorder(default, &mut |e| {
                            super::request_index::rewrite_node(e);
                            super::session_options::rewrite_node(e);
                        });
                    }
                    walk_postorder(&mut action.body, &mut rewrite_narrow_controller_node);
                }
                ControllerBodyItem::Unknown { expr, .. } => {
                    walk_postorder(expr, &mut rewrite_narrow_controller_node);
                }
                _ => {}
            }
        }
    }
    for class in &mut app.library_classes {
        for m in &mut class.methods {
            walk_postorder(&mut m.body, &mut |e| {
                super::session_options::rewrite_node(e);
                super::status_literal::rewrite_node(e);
                super::to_param_residue::rewrite_node(e);
            });
        }
    }
    for model in &mut app.models {
        for item in &mut model.body {
            if let ModelBodyItem::Method { method, .. } = item {
                walk_postorder(&mut method.body, &mut super::to_param_residue::rewrite_node);
            }
        }
    }
}

fn rewrite_narrow_controller_node(e: &mut Expr) {
    super::request_index::rewrite_node(e);
    super::session_options::rewrite_node(e);
    super::status_literal::rewrite_node(e);
    super::to_param_residue::rewrite_node(e);
}

/// Independent rewrites after `tag_builder` and before `kwsplat`: route
/// helpers, enum symbols, `has_json` flatten, and assoc `loaded?`/
/// `.target`. Collect tables once; one owned-hook walk, one view walk,
/// one test walk. Tests skip `position_path_params`, matching the
/// original `route_url_options` surface.
pub fn apply_fused_mid_rewrites(app: &mut App) {
    let helpers = super::route_format_suffix::route_helper_names(app);
    let path_params = if helpers.is_empty() {
        Default::default()
    } else {
        super::route_url_options::helper_path_params(app)
    };
    let enum_map = super::enum_symbols::enum_columns(app);
    let json_map = super::has_json::has_json_columns(&app.models);
    let by_model = super::assoc_loaded::has_many_by_model(app);
    let readers = super::assoc_loaded::association_readers_by_model(app);
    let flat_loaded = super::assoc_loaded::flat_loaded_by_model(app);
    let sole_includer = app.sole_includer_of_modules();

    super::for_each_owned_hook_body(app, &mut |owner, body| {
        walk_postorder(body, &mut |e| {
            rewrite_mid_node(
                e,
                owner,
                &helpers,
                &path_params,
                true,
                &enum_map,
                &json_map,
                &sole_includer,
                &by_model,
                &readers,
                &flat_loaded,
            );
        });
    });
    for view in &mut app.views {
        walk_postorder(&mut view.body, &mut |e| {
            rewrite_mid_node(
                e,
                None,
                &helpers,
                &path_params,
                true,
                &enum_map,
                &json_map,
                &sole_includer,
                &by_model,
                &readers,
                &flat_loaded,
            );
        });
    }
    super::for_each_test_body(app, &mut |body| {
        walk_postorder(body, &mut |e| {
            rewrite_mid_node(
                e,
                None,
                &helpers,
                &path_params,
                false,
                &enum_map,
                &json_map,
                &sole_includer,
                &by_model,
                &readers,
                &flat_loaded,
            );
        });
    });
}

fn rewrite_mid_node(
    e: &mut Expr,
    enclosing: Option<&crate::ident::ClassId>,
    helpers: &std::collections::HashSet<String>,
    path_params: &std::collections::HashMap<String, Vec<String>>,
    position_path: bool,
    enum_map: &super::enum_symbols::EnumMap,
    json_map: &super::has_json::HasJsonColumns,
    sole_includer: &std::collections::HashMap<crate::ident::ClassId, crate::ident::ClassId>,
    by_model: &std::collections::HashMap<crate::ident::ClassId, std::collections::HashSet<Symbol>>,
    readers: &std::collections::HashMap<crate::ident::ClassId, std::collections::HashSet<Symbol>>,
    flat_loaded: &std::collections::HashMap<crate::ident::ClassId, std::collections::HashSet<Symbol>>,
) {
    if !helpers.is_empty() {
        super::route_format_suffix::rewrite_node(e, helpers);
        // Format suffix can wrap a helper as `<call> + ".ext"`. Sequential
        // walks still visit that inner `recv: None` send; a fused callback
        // would otherwise only see `+` and skip position/host rewrites.
        if is_format_suffix_wrap(e, helpers) {
            if let ExprNode::Send { recv: Some(inner), .. } = &mut *e.node {
                apply_route_url_followups(inner, position_path, path_params, helpers);
            }
        } else {
            apply_route_url_followups(e, position_path, path_params, helpers);
        }
    }
    if !enum_map.is_empty() {
        super::enum_symbols::rewrite_node(e, enum_map);
    }
    if !json_map.is_empty() {
        super::has_json::rewrite_node(e, json_map);
    }
    super::assoc_loaded::rewrite_node(e, enclosing, sole_includer, by_model, readers, flat_loaded);
}

fn apply_route_url_followups(
    target: &mut Expr,
    position_path: bool,
    path_params: &std::collections::HashMap<String, Vec<String>>,
    helpers: &std::collections::HashSet<String>,
) {
    if position_path {
        super::route_url_options::rewrite_position_node(target, path_params);
    }
    super::route_url_options::rewrite_node(target, helpers);
}

/// True when `route_format_suffix` just replaced a helper with concat.
fn is_format_suffix_wrap(e: &Expr, helpers: &HashSet<String>) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node else {
        return false;
    };
    if method.as_str() != "+" || args.len() != 1 {
        return false;
    }
    let looks_like_ext = match &*args[0].node {
        ExprNode::Lit { value: Literal::Str { value } } => value.starts_with('.'),
        ExprNode::StringInterp { parts } => {
            matches!(parts.first(), Some(InterpPart::Text { value }) if value == ".")
        }
        _ => false,
    };
    looks_like_ext
        && matches!(
            &*r.node,
            ExprNode::Send { recv: None, method: m, .. } if helpers.contains(m.as_str())
        )
}

fn rewrite_hook_node(
    e: &mut Expr,
    skip_exclude: bool,
    skip_in: bool,
    skip_including: bool,
    skip_full_messages: bool,
) {
    super::pathname_ctor::rewrite_node(e);
    super::array_ordinal::rewrite_node(e);
    super::save_without_validation::rewrite_node(e);
    super::random_formatter::rewrite_node(e);
    super::number_to_fs::rewrite_node(e);
    super::string_inflections::rewrite_node(e);
    super::to_json::rewrite_node(e);
    super::csv_generate::rewrite_node(e);
    super::presence_in::rewrite_node(e);
    super::enumerable_ext::rewrite_node(e);
    super::boolean_cast::rewrite_node(e);
    super::values_at_splat::rewrite_node(e);
    if !skip_exclude {
        super::exclude_predicate::rewrite_node(e);
    }
    if !skip_in {
        super::in_predicate::rewrite_node(e);
    }
    if !skip_including {
        super::including::rewrite_node(e);
    }
    super::exists_conditions::rewrite_node(e);
    super::destroy_by::rewrite_node(e);
    super::literal_append::rewrite_node(e);
    super::byte_size::rewrite_node(e);
    super::dirty_predicate_kwargs::rewrite_node(e);
    super::relation_select_block::rewrite_node(e);
    super::arel_attribute::rewrite_node(e);
    super::attr_or_assign::rewrite_node(e);
    super::group_count::rewrite_node(e);
    if !skip_full_messages {
        super::errors_full_messages::rewrite_node(e);
    }
    super::each_with_index::rewrite_node(e);
    // Last: produces `ActiveSupport.*` / Range nodes no earlier fused
    // rewrite keys on. `where_range_split` still runs sequentially
    // afterwards and walks those Ranges.
    super::time_calendar::rewrite_node(e);
}

fn rewrite_view_node(
    e: &mut Expr,
    skip_exclude: bool,
    skip_in: bool,
    skip_including: bool,
    skip_full_messages: bool,
) {
    super::pathname_ctor::rewrite_node(e);
    super::array_ordinal::rewrite_node(e);
    super::random_formatter::rewrite_node(e);
    super::number_to_fs::rewrite_node(e);
    super::string_inflections::rewrite_node(e);
    super::to_json::rewrite_node(e);
    super::csv_generate::rewrite_node(e);
    super::presence_in::rewrite_node(e);
    super::enumerable_ext::rewrite_node(e);
    super::boolean_cast::rewrite_node(e);
    if !skip_exclude {
        super::exclude_predicate::rewrite_node(e);
    }
    if !skip_in {
        super::in_predicate::rewrite_node(e);
    }
    if !skip_including {
        super::including::rewrite_node(e);
    }
    super::exists_conditions::rewrite_node(e);
    super::destroy_by::rewrite_node(e);
    super::literal_append::rewrite_node(e);
    super::dirty_predicate_kwargs::rewrite_node(e);
    super::relation_select_block::rewrite_node(e);
    super::arel_attribute::rewrite_node(e);
    if !skip_full_messages {
        super::errors_full_messages::rewrite_node(e);
    }
    super::each_with_index::rewrite_node(e);
    super::time_calendar::rewrite_node(e);
}

fn rewrite_test_node(e: &mut Expr, skip_full_messages: bool) {
    super::save_without_validation::rewrite_node(e);
    super::enumerable_ext::rewrite_node(e);
    super::byte_size::rewrite_node(e);
    super::dirty_predicate_kwargs::rewrite_node(e);
    if !skip_full_messages {
        super::errors_full_messages::rewrite_node(e);
    }
}

fn rewrite_late_hook_node(e: &mut Expr) {
    super::rails_cache::rewrite_node(e);
    super::capture_inline::rewrite_node(e);
    super::and_return::rewrite_node(e);
    super::case_lambda::rewrite_node(e);
    super::system_exception::rewrite_node(e);
    super::perform_all_later::rewrite_node(e);
    super::attachables_grep::rewrite_node(e);
    super::send_file::rewrite_node(e);
}

fn walk_postorder(expr: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    expr.node.for_each_child_mut(&mut |c| walk_postorder(c, f));
    f(expr);
}
