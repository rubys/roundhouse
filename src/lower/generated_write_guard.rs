//! Refuse direct writes to generated database columns before persistence and
//! bulk-write lowerings can silently turn them into ordinary inserts/updates.
//!
//! Rails sends `update_column`, `touch(:column)`, and explicit generated keys
//! in `insert_all` directly to the database; generated columns reject those
//! writes. Roundhouse's normal adapter filters generated columns, so leaving
//! these calls to lowerings could make a write disappear. This pass keeps that
//! boundary explicit with a source-located unsupported diagnostic. Normal
//! `save`/`update` assignments remain allowed and retain generated filtering.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

type GeneratedColumns = HashMap<ClassId, HashSet<Symbol>>;

struct Refusal {
    construct: &'static str,
    detail: String,
}

/// Guard direct write APIs on generated columns before the lowering passes
/// rewrite them. Returns source-spanned diagnostics for each refused call.
pub fn apply(app: &mut App) -> Vec<Diagnostic> {
    let generated = generated_columns(app);
    if generated.is_empty() {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    guard_association_touches(app, &generated, &mut diagnostics);
    // A bare call or `self` in a model-owned body has an implicit owning
    // model; its receiver type alone can be absent or `SelfInstance` here.
    super::for_each_model_body_named(app, &mut |owner, body| {
        let owner = ClassId(Symbol::from(owner));
        rewrite(body, Some(&owner), &generated, &mut diagnostics);
    });
    // The owned walk covers scopes/callbacks/defaults and controller/helper
    // receivers. Model bodies already guarded above have no source call left.
    super::for_each_owned_hook_body(app, &mut |owner, body| {
        rewrite(body, owner, &generated, &mut diagnostics);
    });
    // Emit also walks roots the hook pass intentionally excludes: views and
    // normal test modules. The forwarding walk includes hooks too; rejected
    // calls have already been replaced with located stubs, so this second
    // walk cannot duplicate their diagnostics.
    super::for_each_forwarding_body(app, &mut |body| {
        rewrite(body, None, &generated, &mut diagnostics);
    });
    rewrite_emit_only_roots(app, &generated, &mut diagnostics);
    diagnostics
}

/// Mutable roots that the emit survey includes beyond `for_each_forwarding_body`.
/// Keep these aligned with `for_each_emit_body_ref`: expressions in defaults
/// and fixture ERB are emitted even though they are not forwarding bodies.
fn rewrite_emit_only_roots(
    app: &mut App,
    generated: &GeneratedColumns,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for model in &mut app.models {
        let owner = model.name.clone();
        for item in &mut model.body {
            let crate::dialect::ModelBodyItem::Association { assoc, .. } = item else {
                continue;
            };
            match assoc {
                crate::dialect::Association::BelongsTo {
                    default: Some(expr),
                    ..
                } => {
                    rewrite(expr, Some(&owner), generated, diagnostics);
                }
                crate::dialect::Association::HasMany {
                    scope: Some(expr), ..
                } => {
                    rewrite(expr, Some(&owner), generated, diagnostics);
                }
                _ => {}
            }
        }
    }
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            if let crate::dialect::ControllerBodyItem::Action { action, .. } = item {
                for (_, default) in &mut action.kw_params {
                    if let Some(expr) = default {
                        rewrite(expr, None, generated, diagnostics);
                    }
                }
            }
        }
    }
    for view in &mut app.views {
        for default in view
            .strict_locals
            .iter_mut()
            .flatten()
            .filter_map(|param| param.default.as_mut())
        {
            rewrite(default, None, generated, diagnostics);
        }
    }
    for fixture in &mut app.fixtures {
        for expr in &mut fixture.preamble {
            rewrite(expr, None, generated, diagnostics);
        }
        for value in fixture
            .records
            .values_mut()
            .flat_map(|record| record.values_mut())
        {
            if let crate::dialect::FixtureValue::Ruby(expr) = value {
                rewrite(expr, None, generated, diagnostics);
            }
        }
    }
    for helper in &mut app.routes.direct_helpers {
        rewrite(&mut helper.body, None, generated, diagnostics);
    }
    for function in &mut app.sql_functions {
        let mut rewrite_method = |method: &mut crate::dialect::MethodDef| {
            for default in method
                .params
                .iter_mut()
                .filter_map(|param| param.default.as_mut())
            {
                rewrite(default, None, generated, diagnostics);
            }
            rewrite(&mut method.body, None, generated, diagnostics);
        };
        match &mut function.kind {
            crate::app::SqlFunctionKind::Scalar { method } => rewrite_method(method),
            crate::app::SqlFunctionKind::Aggregate { step, finalize } => {
                rewrite_method(step);
                rewrite_method(finalize);
            }
        }
    }
}

/// `belongs_to ..., touch: :column` is lowered later into a setter on
/// the associated record followed by a no-argument `touch`. That path would
/// otherwise pass through the normal adapter, which filters generated fields
/// and silently drops the requested write. Refuse it at the association's
/// source span before callback lowering, and remove the metadata so erroneous
/// output cannot still synthesize the misleading callback.
fn guard_association_touches(
    app: &mut App,
    generated: &GeneratedColumns,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for owner in &mut app.models {
        for item in &mut owner.body {
            let span = item.span();
            let crate::dialect::ModelBodyItem::Association { assoc, .. } = item else {
                continue;
            };
            let detail = match assoc {
                crate::dialect::Association::BelongsTo {
                    name,
                    target,
                    polymorphic,
                    polymorphic_targets,
                    touch: Some(crate::dialect::Touch::Column(column)),
                    ..
                } => {
                    let targets: &[ClassId] = if *polymorphic {
                        polymorphic_targets
                    } else {
                        std::slice::from_ref(target)
                    };
                    let generated_target = targets.iter().find_map(|target| {
                        generated
                            .get(target)
                            .filter(|columns| columns.contains(column))
                            .map(|_| target)
                    });
                    if let Some(target) = generated_target {
                        Some(format!(
                            "`{}#belongs_to :{}` would touch generated column `{}` on `{}`; the database-owned write cannot be preserved",
                            owner.name.0.as_str(),
                            name.as_str(),
                            column.as_str(),
                            target.0.as_str(),
                        ))
                    } else if *polymorphic
                        && polymorphic_targets.is_empty()
                        && generated.values().any(|columns| columns.contains(column))
                    {
                        Some(format!(
                            "`{}#belongs_to :{}` has an unresolved polymorphic target for generated column `{}`; the touch cannot be proven database-safe",
                            owner.name.0.as_str(),
                            name.as_str(),
                            column.as_str(),
                        ))
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(detail) = detail {
                diagnostics.push(Diagnostic::unsupported(
                    span,
                    None,
                    "generated-column association touch",
                    detail,
                ));
                if let crate::dialect::Association::BelongsTo { touch, .. } = assoc {
                    *touch = None;
                }
            }
        }
    }
}

fn generated_columns(app: &App) -> GeneratedColumns {
    app.models
        .iter()
        .filter_map(|model| {
            let table = app.schema.tables.get(&model.table.0)?;
            let columns: HashSet<Symbol> = table
                .columns
                .iter()
                .filter(|column| column.generated.is_some())
                .map(|column| column.name.clone())
                .collect();
            (!columns.is_empty()).then(|| (model.name.clone(), columns))
        })
        .collect()
}

fn rewrite(
    expr: &mut Expr,
    owner: Option<&ClassId>,
    generated: &GeneratedColumns,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(refusal) = generated_write_refusal(expr, owner, generated) {
        let diagnostic =
            Diagnostic::unsupported(expr.span, None, refusal.construct, refusal.detail);
        let mut stub = Expr::new(
            expr.span,
            ExprNode::Lit {
                value: Literal::Nil,
            },
        );
        stub.diagnostic = Some(diagnostic.kind.clone());
        *expr = stub;
        diagnostics.push(diagnostic);
        return;
    }
    expr.node
        .for_each_child_mut(&mut |child| rewrite(child, owner, generated, diagnostics));
}

/// A refusal means the call can write a generated value and cannot be
/// lowered through the ordinary save path without changing its behavior.
/// Dynamic column names fail closed only for a known generated model; literal
/// ordinary columns keep their existing implementation.
fn generated_write_refusal(
    expr: &Expr,
    owner: Option<&ClassId>,
    generated: &GeneratedColumns,
) -> Option<Refusal> {
    let ExprNode::Send {
        recv, method, args, ..
    } = &*expr.node
    else {
        return None;
    };
    let api = method.as_str();

    // `Model.insert_all` currently lowers through ordinary save, which
    // filters generated keys. Guard that class-level inline before the
    // Ruby-family scope pass can transform it; this shared hook also gives
    // strict emitters the same located boundary.
    if matches!(api, "insert_all" | "insert_all!") {
        let model = model_for_bulk_receiver(recv.as_ref(), owner, generated)?;
        return Some(Refusal {
            construct: "generated-column bulk insert",
            detail: format!(
                "`{}.{}` cannot use the per-record save lowering because explicit generated-column keys must reach the database",
                model.0.as_str(),
                api,
            ),
        });
    }

    let is_update_column = api == "update_column" && args.len() == 2;
    let is_touch = api == "touch" && !args.is_empty();
    let is_touching_counter =
        matches!(api, "increment!" | "decrement!") && args.len() == 2 && has_touch_true(&args[1]);
    if !is_update_column && !is_touch && !is_touching_counter {
        return None;
    }

    let models = models_for_receiver(recv.as_ref(), owner, generated);
    if models.is_empty() {
        return None;
    }
    let detail = match api {
        "update_column" => match literal_column_name(&args[0]) {
            Some(column) => models.iter().find_map(|model| {
                generated.get(model).and_then(|columns| {
                    columns.contains(&column).then(|| format!(
                        "`{}#update_column` cannot write generated column `{}`; the database owns that value",
                        model.0.as_str(),
                        column.as_str(),
                    ))
                })
            })?,
            None => format!(
                "`{}#update_column` has a dynamic column key and cannot prove it avoids generated columns",
                model_names(&models),
            ),
        },
        "touch" => match touch_columns(args) {
            Some(columns) => {
                let Some((model, column)) = models.iter().find_map(|model| {
                    let generated_columns = generated.get(model)?;
                    columns
                        .iter()
                        .find(|column| generated_columns.contains(*column))
                        .map(|column| (model, column))
                }) else {
                    return None;
                };
                format!(
                    "`{}#touch` cannot write generated column `{}`; the database owns that value",
                    model.0.as_str(),
                    column.as_str(),
                )
            }
            None => format!(
                "`{}#touch` has a dynamic column name and cannot prove it avoids generated columns",
                model_names(&models),
            ),
        },
        "increment!" | "decrement!" => match literal_column_name(&args[0]) {
            Some(column) => models.iter().find_map(|model| {
                generated.get(model).and_then(|columns| {
                    columns.contains(&column).then(|| format!(
                        "`{}#{}` cannot write generated column `{}`; the database owns that value",
                        model.0.as_str(),
                        api,
                        column.as_str(),
                    ))
                })
            })?,
            None => format!(
                "`{}#{}` has a dynamic column name and cannot prove it avoids generated columns",
                model_names(&models),
                api,
            ),
        },
        _ => return None,
    };
    Some(Refusal {
        construct: "generated-column direct write",
        detail,
    })
}

fn model_for_bulk_receiver(
    recv: Option<&Expr>,
    owner: Option<&ClassId>,
    generated: &GeneratedColumns,
) -> Option<ClassId> {
    match recv {
        None => owner
            .filter(|model| generated.contains_key(*model))
            .cloned(),
        Some(receiver) => match &*receiver.node {
            ExprNode::Const { .. } => model_from_const(receiver, generated),
            ExprNode::SelfRef => owner
                .filter(|model| generated.contains_key(*model))
                .cloned(),
            _ => None,
        },
    }
}

fn models_for_receiver(
    recv: Option<&Expr>,
    owner: Option<&ClassId>,
    generated: &GeneratedColumns,
) -> Vec<ClassId> {
    match recv {
        None => owner
            .filter(|model| generated.contains_key(*model))
            .cloned()
            .into_iter()
            .collect(),
        Some(receiver) if matches!(&*receiver.node, ExprNode::SelfRef) => {
            model_for_self(receiver, owner, generated)
                .into_iter()
                .collect()
        }
        Some(receiver) => model_from_ty(receiver.ty.as_ref(), generated),
    }
}

fn model_for_self(
    receiver: &Expr,
    owner: Option<&ClassId>,
    generated: &GeneratedColumns,
) -> Option<ClassId> {
    owner
        .filter(|model| generated.contains_key(*model))
        .cloned()
        .or_else(|| {
            model_from_ty(receiver.ty.as_ref(), generated)
                .into_iter()
                .next()
        })
}

fn model_from_const(expr: &Expr, generated: &GeneratedColumns) -> Option<ClassId> {
    let ExprNode::Const { path } = &*expr.node else {
        return None;
    };
    let name = ClassId(Symbol::from(
        path.iter()
            .map(|part| part.as_str())
            .collect::<Vec<_>>()
            .join("::"),
    ));
    generated.contains_key(&name).then_some(name)
}

fn model_from_ty(ty: Option<&Ty>, generated: &GeneratedColumns) -> Vec<ClassId> {
    fn collect(ty: &Ty, generated: &GeneratedColumns, out: &mut HashSet<ClassId>) {
        match ty {
            Ty::Class { id, .. } if generated.contains_key(id) => {
                out.insert(id.clone());
            }
            Ty::Union { variants } => {
                for variant in variants {
                    collect(variant, generated, out);
                }
            }
            _ => {}
        }
    }
    let mut out = HashSet::new();
    if let Some(ty) = ty {
        collect(ty, generated, &mut out);
    }
    let mut models: Vec<ClassId> = out.into_iter().collect();
    models.sort();
    models
}

fn model_names(models: &[ClassId]) -> String {
    models
        .iter()
        .map(|model| model.0.as_str())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn literal_column_name(expr: &Expr) -> Option<Symbol> {
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value.clone()),
        ExprNode::Lit {
            value: Literal::Str { value },
        } => Some(Symbol::from(value.as_str())),
        _ => None,
    }
}

/// Recognize named touch columns, allowing Rails' separate `time:` keyword
/// hash. `None` means a dynamic or unsupported key shape that could name a
/// generated column.
fn touch_columns(args: &[Expr]) -> Option<Vec<Symbol>> {
    let mut columns = Vec::new();
    for arg in args {
        if let Some(column) = literal_column_name(arg) {
            columns.push(column);
            continue;
        }
        if is_time_keyword_hash(arg) {
            continue;
        }
        return None;
    }
    Some(columns)
}

fn is_time_keyword_hash(expr: &Expr) -> bool {
    let ExprNode::Hash {
        entries,
        kwargs: true,
    } = &*expr.node
    else {
        return false;
    };
    !entries.is_empty()
        && entries.iter().all(|(key, _)| {
            matches!(&*key.node,
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "time"
            )
        })
}

fn has_touch_true(expr: &Expr) -> bool {
    let ExprNode::Hash { entries, .. } = &*expr.node else {
        return false;
    };
    matches!(
        entries.as_slice(),
        [(key, value)]
            if matches!(&*key.node,
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "touch"
            )
                && matches!(&*value.node,
                    ExprNode::Lit { value: Literal::Bool { value: true } }
                )
    )
}
