// Not a plain Hash read: Rails' `Model.statuses` is indifferent-access, so a Symbol key reads the String label it names.
use std::collections::HashSet;

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_enum_mapping_keys(app: &mut App) {
    let mappings: HashSet<(String, String)> = app
        .models
        .iter()
        .flat_map(|m| {
            let name = m.name.0.as_str().to_string();
            m.enums
                .keys()
                .map(move |col| (name.clone(), crate::naming::pluralize_snake(col.as_str())))
        })
        .collect();
    // Not only `Model.statuses`: inside the model, `statuses`, `self.statuses` and `self.class.statuses` name the same mapping.
    for model in &mut app.models {
        let own: HashSet<String> =
            model.enums.keys().map(|col| crate::naming::pluralize_snake(col.as_str())).collect();
        if own.is_empty() {
            continue;
        }
        for item in &mut model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => {
                    let class_ctx = matches!(method.receiver, crate::dialect::MethodReceiver::Class);
                    rewrite(&mut method.body, &mappings, Some((&own, class_ctx)));
                }
                crate::dialect::ModelBodyItem::Scope { scope, .. } => {
                    rewrite(&mut scope.body, &mappings, Some((&own, true)));
                }
                _ => {}
            }
        }
    }
    super::for_each_hook_body(app, &mut |body| rewrite(body, &mappings, None));
    super::for_each_test_body(app, &mut |body| rewrite(body, &mappings, None));
    for view in &mut app.views {
        rewrite(&mut view.body, &mappings, None);
    }
}

fn rewrite(expr: &mut Expr, mappings: &HashSet<(String, String)>, own: Option<(&HashSet<String>, bool)>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, mappings, own));
    let ExprNode::Send {
        recv: Some(r),
        method,
        args,
        ..
    } = &mut *expr.node
    else {
        return;
    };
    if !matches!(
        method.as_str(),
        "[]" | "fetch" | "key?" | "has_key?" | "include?" | "member?" | "dig" | "except" | "slice"
    ) || !(is_enum_mapping(r, mappings) || own.is_some_and(|(plurals, class_ctx)| is_own_mapping(r, plurals, class_ctx)))
    {
        return;
    }
    let keyed = if matches!(method.as_str(), "except" | "slice") {
        args.len()
    } else {
        args.len().min(1)
    };
    for arg in args.iter_mut().take(keyed) {
        if matches!(arg.ty, Some(Ty::Str))
            || matches!(
                &*arg.node,
                ExprNode::Lit {
                    value: crate::expr::Literal::Str { .. }
                }
            )
        {
            continue;
        }
        let span = arg.span;
        if let ExprNode::Lit { value: crate::expr::Literal::Sym { value } } = &*arg.node {
            let label = value.as_str().to_string();
            *arg = Expr::new(span, ExprNode::Lit { value: crate::expr::Literal::Str { value: label } });
            arg.ty = Some(Ty::Str);
            continue;
        }
        let inner = std::mem::replace(arg, Expr::new(span, ExprNode::Seq { exprs: vec![] }));
        let mut call = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(inner),
                method: Symbol::from("to_s"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        call.ty = Some(Ty::Str);
        *arg = call;
    }
}

fn is_enum_mapping(e: &Expr, mappings: &HashSet<(String, String)>) -> bool {
    let ExprNode::Send {
        recv: Some(c),
        method,
        args,
        block: None,
        ..
    } = &*e.node
    else {
        return false;
    };
    let ExprNode::Const { path } = &*c.node else {
        return false;
    };
    let model = path
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("::");
    args.is_empty() && mappings.contains(&(model, method.as_str().to_string()))
}

fn is_own_mapping(e: &Expr, plurals: &HashSet<String>, class_ctx: bool) -> bool {
    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else {
        return false;
    };
    if !args.is_empty() || !plurals.contains(method.as_str()) {
        return false;
    }
    let is_self = |r: &Option<Expr>| match r {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef),
    };
    match recv {
        Some(r) if matches!(&*r.node, ExprNode::Send { method, args, .. } if method.as_str() == "class" && args.is_empty()) => {
            let ExprNode::Send { recv: inner, .. } = &*r.node else { unreachable!() };
            is_self(inner)
        }
        _ => class_ctx && is_self(recv),
    }
}
