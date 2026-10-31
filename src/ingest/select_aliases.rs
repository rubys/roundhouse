use crate::app::App;
use crate::dialect::{Association, ControllerBodyItem, MethodReceiver, Model, SelectAlias};
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::{ClassId, Symbol};

const RELATION_HOPS: &[&str] = &[
    "all", "where", "not", "rewhere", "joins", "left_joins", "left_outer_joins", "includes",
    "preload", "eager_load", "references", "group", "regroup", "having", "order", "reorder",
    "in_order_of", "limit", "offset", "distinct", "select", "reselect", "unscoped", "unscope",
    "readonly", "lock", "none", "excluding", "without", "and", "or", "merge", "extending",
    "strict_loading",
];

pub(super) fn stamp(app: &mut App) {
    let mut found: Vec<(ClassId, SelectAlias)> = Vec::new();
    {
        let models = &app.models;
        let mut visit = |expr: &Expr, ctx: Option<&ClassId>| collect(expr, ctx, models, &mut found);
        for model in models {
            for item in &model.body {
                match item {
                    crate::dialect::ModelBodyItem::Association { assoc, .. } => {
                        if let Association::HasMany { scope: Some(scope), target, .. }
                        | Association::HasOne { scope: Some(scope), target, .. } = assoc
                        {
                            visit(scope, Some(target));
                        }
                    }
                    crate::dialect::ModelBodyItem::Scope { scope, .. } => visit(&scope.body, Some(&model.name)),
                    crate::dialect::ModelBodyItem::Method { method, .. } => {
                        let ctx = (method.receiver == MethodReceiver::Class).then_some(&model.name);
                        visit(&method.body, ctx);
                    }
                    _ => {}
                }
            }
        }
        for class in &app.library_classes {
            for method in &class.methods {
                visit(&method.body, None);
            }
        }
        for controller in &app.controllers {
            for item in &controller.body {
                match item {
                    ControllerBodyItem::Action { action, .. } => visit(&action.body, None),
                    ControllerBodyItem::ClassMethod { method, .. } => visit(&method.body, None),
                    _ => {}
                }
            }
        }
        for view in &app.views {
            visit(&view.body, None);
        }
    }
    let methods = crate::analyze::SourceMethodIndex::new(app);
    let found: Vec<(ClassId, SelectAlias)> = found
        .into_iter()
        .map(|(class, alias)| {
            let source_method = methods.instance_method(&class, alias.name.as_str());
            (class, SelectAlias { source_method, ..alias })
        })
        .collect();
    for (class, alias) in found {
        let Some(model) = app.models.iter_mut().find(|m| m.name == class) else { continue };
        if !model.select_aliases.iter().any(|a| a.name == alias.name && a.span == alias.span) {
            model.select_aliases.push(alias);
        }
    }
}

fn collect(expr: &Expr, ctx: Option<&ClassId>, models: &[Model], out: &mut Vec<(ClassId, SelectAlias)>) {
    if let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node
        && method.as_str() == "select"
        && !args.is_empty()
        && let Some(model) = relation_model(recv.as_ref(), ctx, models)
    {
        for arg in args {
            let Some(sql) = sql_text(arg) else { continue };
            for name in aliases(&sql) {
                out.push((model.clone(), SelectAlias { name: Symbol::from(name), span: expr.span, source_method: false }));
            }
        }
    }
    expr.node.for_each_child(&mut |child| collect(child, ctx, models, out));
}

fn relation_model(recv: Option<&Expr>, ctx: Option<&ClassId>, models: &[Model]) -> Option<ClassId> {
    let mut hops: Vec<&Symbol> = Vec::new();
    let mut cur = recv;
    let root = loop {
        let Some(e) = cur else { break ctx.cloned()? };
        match &*e.node {
            ExprNode::SelfRef => break ctx.cloned()?,
            ExprNode::Const { path } => break model_named(path, models)?,
            ExprNode::Send { recv, method, .. } => {
                hops.push(method);
                cur = recv.as_ref();
            }
            _ => return None,
        }
    };
    let model = models.iter().find(|m| m.name == root)?;
    hops.iter()
        .all(|hop| RELATION_HOPS.contains(&hop.as_str()) || model.scopes().any(|s| &s.name == *hop))
        .then_some(root)
}

fn model_named(path: &[Symbol], models: &[Model]) -> Option<ClassId> {
    let full = path.iter().map(Symbol::as_str).collect::<Vec<_>>().join("::");
    let full = full.trim_start_matches("::");
    models.iter().find(|m| m.name.0.as_str() == full).map(|m| m.name.clone())
}

// An interpolation is not dropped: it can only stand where a value does, never an alias.
fn sql_text(arg: &Expr) -> Option<String> {
    match &*arg.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::StringInterp { parts } => Some(
            parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => value.clone(),
                    InterpPart::Expr { .. } => "0".to_string(),
                })
                .collect(),
        ),
        _ => None,
    }
}

pub fn aliases(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    for item in split_top_level(sql) {
        if let Some(name) = item_alias(&item) {
            out.push(name);
        }
    }
    out
}

fn split_top_level(sql: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for c in sql.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' | '`' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    items.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            },
        }
        cur.push(c);
    }
    items.push(cur);
    items
}

fn item_alias(item: &str) -> Option<String> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut last_as: Option<usize> = None;
    let bytes: Vec<char> = item.chars().collect();
    for (i, &c) in bytes.iter().enumerate() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => quote = Some(c),
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            'a' | 'A' if depth == 0 => {
                let before_ws = i > 0 && bytes[i - 1].is_whitespace() || i > 0 && bytes[i - 1] == ')';
                let s_next = bytes.get(i + 1).is_some_and(|n| *n == 's' || *n == 'S');
                let after_ws = bytes.get(i + 2).is_some_and(|n| n.is_whitespace() || *n == '"' || *n == '`');
                if before_ws && s_next && after_ws {
                    last_as = Some(i + 2);
                }
            }
            _ => {}
        }
    }
    let rest: String = bytes[last_as?..].iter().collect();
    let name = rest.trim().trim_matches(|c| c == '"' || c == '`');
    let mut chars = name.chars();
    let first = chars.next()?;
    ((first.is_ascii_alphabetic() || first == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::aliases;

    #[test]
    fn names_each_top_level_alias() {
        assert_eq!(aliases("count(*) as times, title"), vec!["times"]);
        assert_eq!(
            aliases("comment_id, reason, count(1) as count, group_concat(username, ', ') as usernames"),
            vec!["count", "usernames"]
        );
        assert_eq!(
            aliases("users.id, (sum(flags) - 0)/0 as sigma,\n count(distinct if(flags > 0, comments.id, null)) AS n_comments,\n (\n count(x) * 100.0 /\n count(y)\n ) as \"percent_flagged\""),
            vec!["sigma", "n_comments", "percent_flagged"]
        );
        assert!(aliases("cast(x as integer), 'a as b', title").is_empty());
        assert!(aliases("users.*").is_empty());
    }
}
