use std::collections::{BTreeMap, BTreeSet};

use crate::dialect::Controller;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

use super::ParamsSpecs;

fn walk(expr: &Expr, visit: &mut impl FnMut(&Expr)) {
    visit(expr);
    expr.node.for_each_child(&mut |child| walk(child, visit));
}

fn own_call(expr: &Expr) -> Option<(&Symbol, &[Expr])> {
    let ExprNode::Send {
        recv, method, args, ..
    } = &*expr.node
    else {
        return None;
    };
    recv.as_ref()
        .is_none_or(|recv| matches!(&*recv.node, ExprNode::SelfRef))
        .then_some((method, args))
}

fn uploaded_source(expr: &Expr) -> Option<&Expr> {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        block: Some(block),
        ..
    } = &*expr.node
    else {
        return None;
    };
    if !matches!(method.as_str(), "map" | "each") {
        return None;
    }
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return None;
    };
    let [file] = params.as_slice() else {
        return None;
    };
    let source = match &*recv.node {
        ExprNode::Var { .. } => recv,
        ExprNode::Send {
            recv: None,
            method,
            args,
            ..
        } if method.as_str() == "Array" => {
            let [arg] = args.as_slice() else { return None };
            arg
        }
        _ => recv,
    };
    let mut reads = BTreeSet::new();
    walk(body, &mut |expr| {
        if let ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            ..
        } = &*expr.node
            && args.is_empty()
            && matches!(&*recv.node, ExprNode::Var { name, .. } if name == file)
        {
            reads.insert(method.as_str().to_string());
        }
    });
    (reads.contains("read")
        || reads.contains("original_filename")
        || reads.contains("content_type"))
    .then_some(source)
}

pub(super) fn mark(controllers: &[Controller], specs: &mut ParamsSpecs) {
    let mut fields = BTreeSet::new();
    for controller in controllers {
        let mut upload_params: BTreeMap<Symbol, BTreeSet<usize>> = BTreeMap::new();
        for action in controller.actions() {
            if !action.opt_params.is_empty()
                || !action.kw_params.is_empty()
                || action.rest_param.is_some()
            {
                continue;
            }
            let names: Vec<_> = action.params.fields.keys().collect();
            walk(&action.body, &mut |expr| {
                if let Some(source) = uploaded_source(expr)
                    && let ExprNode::Var { name, .. } = &*source.node
                    && let Some(index) = names.iter().position(|param| *param == name)
                {
                    upload_params
                        .entry(action.name.clone())
                        .or_default()
                        .insert(index);
                }
            });
        }
        for action in controller.actions() {
            walk(&action.body, &mut |expr| {
                let mut sources = Vec::new();
                if let Some(source) = uploaded_source(expr) {
                    sources.push(source);
                }
                if let Some((method, args)) = own_call(expr)
                    && let Some(indices) = upload_params.get(method)
                {
                    sources.extend(indices.iter().filter_map(|index| args.get(*index)));
                }
                for arg in sources {
                    let ExprNode::Send { method, args, .. } = &*arg.node else {
                        continue;
                    };
                    if method.as_str() != "[]" {
                        continue;
                    }
                    let [key] = args.as_slice() else { continue };
                    let field = match &*key.node {
                        ExprNode::Lit {
                            value: Literal::Sym { value },
                        } => value.clone(),
                        ExprNode::Lit {
                            value: Literal::Str { value },
                        } => Symbol::from(value.as_str()),
                        _ => continue,
                    };
                    fields.insert(field);
                }
            });
        }
    }
    for spec in &mut specs.specs {
        for key in &spec.nested_candidates {
            if key.mode == "scalars" && fields.contains(&key.name) {
                spec.file_array_fields.insert(key.name.clone());
            }
        }
    }
}
