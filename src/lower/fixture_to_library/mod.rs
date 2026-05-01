//! Lower YAML fixtures into one `<Plural>Fixtures` LibraryClass per
//! file. Each labeled record becomes `def self.<label>` returning a
//! typed model instance built via the model's `.new({field: value,
//! ...})` constructor.
//!
//! Companion rewrite: `articles(:one)` calls in test bodies get
//! rewritten to `ArticlesFixtures.one()`. Self-describing — the call
//! site lands at concrete dispatch (no runtime fixture-lookup helper
//! needed) and types through the registry like any other class call.
//!
//! IDs: assigned 1-indexed within each fixture file, mirroring
//! Rails's AUTOINCREMENT-on-load behavior. Predictable so test
//! setups can `Article.find(1)` if they need to.

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::lower::fixtures::{
    LoweredFixture, LoweredFixtureRecord, LoweredFixtureSet, LoweredFixtureValue,
};
use crate::lower::typing::{fn_sig, lit_int, lit_str, lit_sym, with_ty};
use crate::naming::camelize;
use crate::span::Span;
use crate::ty::Ty;
use crate::App;

/// Bulk entry. Lower every fixture file into a `<Plural>Fixtures`
/// LibraryClass. Returns an empty Vec when the app has no fixtures
/// (apps without test fixtures skip the artifact).
pub fn lower_fixtures_to_library_classes(app: &App) -> Vec<LibraryClass> {
    let lowered = crate::lower::lower_fixtures(app);
    lowered
        .fixtures
        .iter()
        .map(|f| build_fixture_class(f, &lowered))
        .collect()
}

fn build_fixture_class(f: &LoweredFixture, all: &LoweredFixtureSet) -> LibraryClass {
    let owner_name = format!("{}Fixtures", camelize(f.name.as_str()));
    let owner_id = ClassId(Symbol::from(owner_name.clone()));
    let class_ty = Ty::Class { id: f.class.clone(), args: vec![] };

    let methods: Vec<MethodDef> = f
        .records
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            let id = (idx + 1) as i64;
            let body = build_constructor_call(&f.class, id, r, all);
            MethodDef {
                name: r.label.clone(),
                receiver: MethodReceiver::Class,
                params: Vec::new(),
                body,
                signature: Some(fn_sig(vec![], class_ty.clone())),
                effects: EffectSet::default(),
                enclosing_class: Some(owner_id.0.clone()),
                kind: AccessorKind::Method,
            }
        })
        .collect();

    LibraryClass {
        name: owner_id,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods,
    }
}

/// Build `<Class>.new({id: <id>, <field>: <value>, ...})`. Fields
/// resolve through the lowered record + cross-fixture FK lookups.
fn build_constructor_call(
    cls: &ClassId,
    id: i64,
    record: &LoweredFixtureRecord,
    all: &LoweredFixtureSet,
) -> Expr {
    let mut entries: Vec<(Expr, Expr)> = Vec::new();
    entries.push((lit_sym(Symbol::from("id")), lit_int(id)));
    for field in &record.fields {
        let value_expr = match &field.value {
            LoweredFixtureValue::Literal { ty, raw } => literal_value_to_expr(ty, raw),
            LoweredFixtureValue::FkLookup { target_fixture, target_label } => {
                resolve_fk_id(target_fixture, target_label, all)
            }
        };
        entries.push((lit_sym(field.column.clone()), value_expr));
    }
    let hash_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let hash_expr = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries, braced: true },
        ),
        hash_ty,
    );
    let class_const = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![cls.0.clone()] },
        ),
        Ty::Class { id: cls.clone(), args: vec![] },
    );
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(class_const),
                method: Symbol::from("new"),
                args: vec![hash_expr],
                block: None,
                parenthesized: true,
            },
        ),
        Ty::Class { id: cls.clone(), args: vec![] },
    )
}

/// YAML-string values come through as raw strings; cast to the
/// column's typed literal. Number-shaped raws to Int/Float; "true"/
/// "false" to Bool; everything else to Str.
fn literal_value_to_expr(ty: &Ty, raw: &str) -> Expr {
    match ty {
        Ty::Int => raw
            .parse::<i64>()
            .map(lit_int)
            .unwrap_or_else(|_| lit_str(raw.to_string())),
        Ty::Float => raw
            .parse::<f64>()
            .map(|v| with_ty(
                Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value: v } }),
                Ty::Float,
            ))
            .unwrap_or_else(|_| lit_str(raw.to_string())),
        Ty::Bool => match raw {
            "true" => with_ty(
                Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value: true } }),
                Ty::Bool,
            ),
            "false" => with_ty(
                Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value: false } }),
                Ty::Bool,
            ),
            _ => lit_str(raw.to_string()),
        },
        // Str / Sym / Time / everything else: render as String literal
        // (Time columns get ISO strings from YAML; the model's accessor
        // typing presents them as String anyway).
        _ => lit_str(raw.to_string()),
    }
}

/// FK resolution: find the target fixture's record by label,
/// substitute its 1-indexed position as the literal Int id.
fn resolve_fk_id(
    target_fixture: &Symbol,
    target_label: &Symbol,
    all: &LoweredFixtureSet,
) -> Expr {
    if let Some(target) = all.fixtures.iter().find(|f| &f.name == target_fixture) {
        if let Some((idx, _)) = target
            .records
            .iter()
            .enumerate()
            .find(|(_, r)| &r.label == target_label)
        {
            return lit_int((idx + 1) as i64);
        }
    }
    // Fallback: missing reference — emit 0 so the IR is still typed.
    // The runtime will likely fail when looking up id=0, surfacing
    // the broken FK at test time rather than emit time.
    lit_int(0)
}

/// Walk each test method body and rewrite `<fixture_name>(:label)`
/// bare-Sends to `<Plural>Fixtures.<label>()` Const-Sends. Lets the
/// body-typer dispatch through the fixture class registry without a
/// runtime fixture-lookup helper.
///
/// Called by `test_module_to_library` on each method body before
/// typing. Takes the App's fixtures slice (uses fixture names as
/// the key set) plus an optional model lookup for the class name.
pub fn rewrite_fixture_calls(body: &Expr, fixture_names: &[Symbol]) -> Expr {
    map_expr(body, &|e| {
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
        if !fixture_names.iter().any(|f| f == method) {
            return None;
        }
        if args.len() != 1 {
            return None;
        }
        let ExprNode::Lit { value: Literal::Sym { value: label } } = &*args[0].node else {
            return None;
        };
        let owner = format!("{}Fixtures", camelize(method.as_str()));
        let owner_id = ClassId(Symbol::from(owner.clone()));
        let class_const = with_ty(
            Expr::new(
                e.span,
                ExprNode::Const { path: vec![Symbol::from(owner)] },
            ),
            Ty::Class { id: owner_id, args: vec![] },
        );
        Some(Expr::new(
            e.span,
            ExprNode::Send {
                recv: Some(class_const),
                method: label.clone(),
                args: vec![],
                block: None,
                parenthesized: true,
            },
        ))
    })
}

/// Minimal map_expr — bottom-up rewrite. Returns Some(replacement)
/// to substitute, None to descend unchanged. Modeled on the pattern
/// in `controller_to_library/rewrites.rs`; duplicated here to keep
/// the lowerer self-contained.
fn map_expr(e: &Expr, f: &dyn Fn(&Expr) -> Option<Expr>) -> Expr {
    let mapped = match &*e.node {
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(|r| map_expr(r, f)),
            method: method.clone(),
            args: args.iter().map(|a| map_expr(a, f)).collect(),
            block: block.as_ref().map(|b| map_expr(b, f)),
            parenthesized: *parenthesized,
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: map_expr(fun, f),
            args: args.iter().map(|a| map_expr(a, f)).collect(),
            block: block.as_ref().map(|b| map_expr(b, f)),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: map_expr(body, f),
            block_style: *block_style,
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: map_expr(cond, f),
            then_branch: map_expr(then_branch, f),
            else_branch: map_expr(else_branch, f),
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|c| map_expr(c, f)).collect(),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: map_expr(left, f),
            right: map_expr(right, f),
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (map_expr(k, f), map_expr(v, f)))
                .collect(),
            braced: *braced,
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(|x| map_expr(x, f)).collect(),
            style: *style,
        },
        ExprNode::Case { scrutinee, arms } => ExprNode::Case {
            scrutinee: map_expr(scrutinee, f),
            arms: arms
                .iter()
                .map(|a| crate::expr::Arm {
                    pattern: a.pattern.clone(),
                    guard: a.guard.as_ref().map(|g| map_expr(g, f)),
                    body: map_expr(&a.body, f),
                })
                .collect(),
        },
        ExprNode::Assign { target, value } => ExprNode::Assign {
            target: match target {
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: map_expr(recv, f),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: map_expr(recv, f),
                    index: map_expr(index, f),
                },
                other => other.clone(),
            },
            value: map_expr(value, f),
        },
        ExprNode::Let { id, name, value, body } => ExprNode::Let {
            id: *id,
            name: name.clone(),
            value: map_expr(value, f),
            body: map_expr(body, f),
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    InterpPart::Expr { expr } => InterpPart::Expr {
                        expr: map_expr(expr, f),
                    },
                })
                .collect(),
        },
        ExprNode::Return { value } => ExprNode::Return { value: map_expr(value, f) },
        ExprNode::Raise { value } => ExprNode::Raise { value: map_expr(value, f) },
        ExprNode::Yield { args } => ExprNode::Yield {
            args: args.iter().map(|a| map_expr(a, f)).collect(),
        },
        // Leaves and other composites pass through.
        _ => return f(e).unwrap_or_else(|| e.clone()),
    };
    let new_e = Expr {
        span: e.span,
        node: Box::new(mapped),
        ty: e.ty.clone(),
        effects: e.effects.clone(),
        leading_blank_line: e.leading_blank_line,
        diagnostic: e.diagnostic.clone(),
    };
    f(&new_e).unwrap_or(new_e)
}
