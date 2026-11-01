// Not left as a call: `read_attribute`/`write_attribute` are Rails' spellings of a model's `[]`/`[]=`, and no runtime defines them by those names.
use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub(super) mod receiver;

/// The app's models, and those whose `[]` / `[]=` the app writes itself.
pub(crate) struct Models {
    names: crate::det::HashSet<String>,
    own_reader: crate::det::HashSet<String>,
    own_writer: crate::det::HashSet<String>,
}

impl Models {
    pub(crate) fn of(app: &App) -> Self {
        let methods = crate::analyze::SourceMethodIndex::new(app);
        let defining = |name| app.models.iter()
            .filter(|m| methods.instance_method(&m.name, name))
            .map(|m| m.name.0.as_str().to_string()).collect();
        let names = app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
        Models { names, own_reader: defining("[]"), own_writer: defining("[]=") }
    }
}

#[allow(dead_code)]
pub fn apply_attribute_alias_lowering(app: &mut App) {
    receiver::prove(app);
    let models = Models::of(app);
    for model in &mut app.models {
        for item in &mut model.body {
            use crate::dialect::{Association, ModelBodyItem};
            match item {
                ModelBodyItem::Method { method, .. } => rewrite(&mut method.body, &models, true),
                ModelBodyItem::Scope { scope, .. } => rewrite(&mut scope.body, &models, true),
                ModelBodyItem::Callback { callback, .. } => {
                    if let Some(cond) = &mut callback.condition {
                        rewrite(cond, &models, true);
                    }
                }
                ModelBodyItem::Validation { validation, .. } => {
                    if let Some(cond) = &mut validation.if_cond {
                        rewrite(cond, &models, true);
                    }
                    if let Some(cond) = &mut validation.unless_cond {
                        rewrite(cond, &models, true);
                    }
                }
                ModelBodyItem::Unknown { expr, .. } => rewrite(expr, &models, true),
                ModelBodyItem::Association { assoc: Association::HasMany { extension, .. }, .. } => {
                    for m in extension.iter_mut() {
                        rewrite(&mut m.body, &models, true);
                    }
                }
                _ => {}
            }
        }
    }
    let mut outside = |e: &mut Expr| rewrite(e, &models, false);
    super::for_each_hook_body(app, &mut outside);
    super::for_each_test_body(app, &mut outside);
    for view in &mut app.views {
        rewrite(&mut view.body, &models, false);
    }
}

fn rewrite(expr: &mut Expr, models: &Models, in_model: bool) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, models, in_model));
    rewrite_node(expr, models, in_model);
}

pub(crate) fn rewrite_node(expr: &mut Expr, models: &Models, in_model: bool) {
    let ExprNode::Send { recv, method, args, block: None, .. } = &mut *expr.node else { return };
    let to = match (method.as_str(), args.len()) {
        ("read_attribute", 1) => "[]",
        ("write_attribute", 2) => "[]=",
        ("[]", 1) | ("[]=", 2) => {
            string_key_as_symbol(recv.as_ref(), method, args, models);
            return;
        }
        _ => return,
    };
    let on_model = match recv {
        None => in_model,
        // Not only a typed model: a test body's locals reach this pass untyped, and these names are ActiveRecord's alone.
        Some(r) => match r.ty.as_ref() {
            None => true,
            Some(Ty::Class { id, .. }) => models.names.contains(id.0.as_str()),
            Some(_) => false,
        },
    };
    if !on_model {
        return;
    }
    symbol_key(&mut args[0]);
    if recv.is_none() {
        *recv = Some(Expr::new(expr.span, ExprNode::SelfRef));
    }
    *method = Symbol::from(to);
}

/// `record["title"]` is `record[:title]`: literal String keys lower to
/// Symbols to fit the synthesized indexers' monomorphic signatures.
/// Only on a receiver typed as an app model instance
/// whose indexer is that synthesized one: a Hash, an untyped value or
/// an app-defined `[]` keeps the String it was given.
fn string_key_as_symbol(recv: Option<&Expr>, method: &Symbol, args: &mut [Expr], models: &Models) {
    // Absence of exact class identity is not instance evidence: `.class`,
    // aliases of it and app helpers can all have the same nominal type.
    if !recv.is_some_and(|r| {
        r.decisions & crate::expr::INDEXER_INSTANCE_VALUE != 0
            && r.decisions & (crate::expr::CLASS_OBJECT_VALUE | crate::expr::INDEXER_CLASS_QUERY_VALUE) == 0
    }) {
        return;
    }
    let Some(Ty::Class { id, .. }) = recv.and_then(|r| r.ty.as_ref()).map(Ty::peel_nilable) else { return };
    let own = if method.as_str() == "[]" { &models.own_reader } else { &models.own_writer };
    if models.names.contains(id.0.as_str()) && !own.contains(id.0.as_str()) {
        symbol_key(&mut args[0]);
    }
}

fn symbol_key(arg: &mut Expr) {
    if let ExprNode::Lit { value: Literal::Str { value } } = &*arg.node {
        let name = Symbol::from(value.as_str());
        *arg.node = ExprNode::Lit { value: Literal::Sym { value: name } };
        arg.ty = Some(Ty::Sym);
    }
}
