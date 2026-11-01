//! Value-side evidence for literal model-indexer keys. Deliberately separate
//! from nominal types and from the analyzer's exact class-identity facts.
use crate::app::App;
use crate::det::{HashMap, HashSet};
use crate::dialect::{MethodDef, MethodReceiver};
use crate::expr::{
    CLASS_OBJECT_VALUE, Expr, ExprNode, INDEXER_CLASS_QUERY_VALUE, INDEXER_INSTANCE_VALUE, LValue,
};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Side {
    #[default]
    Unknown,
    Instance,
    Class,
}

#[derive(Clone, Default)]
struct Scope {
    owner: Option<ClassId>,
    self_side: Side,
    locals: HashMap<Symbol, Side>,
    returns: Vec<Side>,
}

type Method = (ClassId, bool, Symbol);
struct Facts {
    class_returns: HashSet<Method>,
    overrides: HashSet<Method>,
    models: HashSet<ClassId>,
}

// Only framework operations that produce records, with source overrides
// excluded below. Inferred return types alone never grant this evidence.
const RECORDS: &[&str] = &[
    "new", "create", "create!", "find", "find_by", "find_by!", "first", "first!", "last", "last!",
    "take", "take!", "sole",
];
const ITERATORS: &[&str] = &["each", "map", "collect", "find_each"];

pub(in crate::lower) fn prove(app: &mut App) {
    let source = crate::analyze::SourceMethodIndex::new(app);
    let mut overrides = HashSet::default();
    for owner in app
        .models
        .iter()
        .map(|m| &m.name)
        .chain(app.library_classes.iter().map(|c| &c.name))
    {
        for name in RECORDS.iter().chain(ITERATORS).chain(["class"].iter()) {
            for class in [false, true] {
                let exists = if class {
                    source.class_method(owner, name)
                } else {
                    source.instance_method(owner, name)
                };
                if exists {
                    overrides.insert((owner.clone(), class, Symbol::from(*name)));
                }
            }
        }
    }
    let mut facts = Facts {
        class_returns: HashSet::default(),
        overrides,
        models: app.models.iter().map(|m| m.name.clone()).collect(),
    };
    // Monotone query summaries: recursive/unresolved helpers remain unknown.
    // No helper is assumed to return an instance from its nominal signature.
    loop {
        let mut found = HashSet::default();
        for model in &mut app.models {
            for item in &mut model.body {
                if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                    summarize(&model.name, method, &facts, &mut found);
                }
            }
        }
        for class in &mut app.library_classes {
            for method in &mut class.methods {
                summarize(&class.name, method, &facts, &mut found);
            }
        }
        let before = facts.class_returns.len();
        facts.class_returns.extend(found);
        if facts.class_returns.len() == before {
            break;
        }
    }
    let mut unknown = |body: &mut Expr| {
        walk(body, &mut Scope::default(), &facts);
    };
    super::super::for_each_hook_body(app, &mut unknown);
    super::super::for_each_test_body(app, &mut unknown);
    for view in &mut app.views {
        unknown(&mut view.body);
    }
    // Restore lexical self evidence after the generic hook walk, which also
    // covers initializers, seeds, defaults and other bodies of unknown side.
    for model in &mut app.models {
        for item in &mut model.body {
            if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                summarize(&model.name, method, &facts, &mut HashSet::default());
            }
        }
    }
    for class in &mut app.library_classes {
        for method in &mut class.methods {
            summarize(&class.name, method, &facts, &mut HashSet::default());
        }
    }
}

fn summarize(owner: &ClassId, method: &mut MethodDef, facts: &Facts, found: &mut HashSet<Method>) {
    let class = method.receiver == MethodReceiver::Class;
    let mut scope = Scope {
        owner: Some(owner.clone()),
        // A module's instance method may be extended onto a class object.
        // Only a model's own instance body establishes a record self here.
        self_side: if class {
            Side::Class
        } else if facts.models.contains(owner) {
            Side::Instance
        } else {
            Side::Unknown
        },
        ..Scope::default()
    };
    let tail = walk(&mut method.body, &mut scope, facts);
    if tail == Side::Class && scope.returns.iter().all(|s| *s == Side::Class) {
        found.insert((owner.clone(), class, method.name.clone()));
    }
}

fn owner(expr: &Expr) -> Option<&ClassId> {
    match expr.ty.as_ref().map(Ty::peel_nilable) {
        Some(Ty::Class { id, .. }) => Some(id),
        Some(Ty::Relation { of }) => Some(of),
        _ => None,
    }
}

fn overridden(facts: &Facts, recv: &Expr, class: bool, method: &Symbol) -> bool {
    owner(recv).is_none_or(|id| {
        facts
            .overrides
            .contains(&(id.clone(), class, method.clone()))
    })
}

fn record_source(facts: &Facts, recv: &Expr) -> bool {
    match recv.ty.as_ref().map(Ty::peel_nilable) {
        Some(Ty::Relation { of }) => facts.models.contains(of),
        Some(Ty::Class { id, .. }) => {
            facts.models.contains(id) && recv.decisions & CLASS_OBJECT_VALUE != 0
        }
        _ => false,
    }
}

fn forget_writes(expr: &Expr, scope: &mut Scope) {
    match &*expr.node {
        ExprNode::Assign {
            target: LValue::Var { name, .. },
            ..
        }
        | ExprNode::OpAssign {
            target: LValue::Var { name, .. },
            ..
        }
        | ExprNode::Let { name, .. } => {
            scope.locals.remove(name);
        }
        ExprNode::MultiAssign { targets, .. } => {
            for t in targets {
                if let LValue::Var { name, .. } = t {
                    scope.locals.remove(name);
                }
            }
        }
        _ => {}
    }
    expr.node
        .for_each_child(&mut |child| forget_writes(child, scope));
}

fn walk(expr: &mut Expr, scope: &mut Scope, facts: &Facts) -> Side {
    let side = match &mut *expr.node {
        ExprNode::SelfRef => scope.self_side,
        ExprNode::Const { .. } if expr.decisions & CLASS_OBJECT_VALUE != 0 => Side::Class,
        ExprNode::Var { name, .. } => scope.locals.get(name).copied().unwrap_or_default(),
        ExprNode::Seq { exprs } => exprs
            .iter_mut()
            .map(|e| walk(e, scope, facts))
            .last()
            .unwrap_or_default(),
        ExprNode::Assign {
            target: LValue::Var { name, .. },
            value,
        } => {
            let side = walk(value, scope, facts);
            scope.locals.insert(name.clone(), side);
            side
        }
        ExprNode::Let {
            name, value, body, ..
        } => {
            let side = walk(value, scope, facts);
            let mut inner = scope.clone();
            inner.locals.insert(name.clone(), side);
            let result = walk(body, &mut inner, facts);
            scope
                .returns
                .extend(inner.returns.into_iter().skip(scope.returns.len()));
            forget_writes(body, scope);
            result
        }
        ExprNode::Return { value } => {
            let side = walk(value, scope, facts);
            scope.returns.push(side);
            side
        }
        ExprNode::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            walk(cond, scope, facts);
            let mut left = scope.clone();
            let mut right = scope.clone();
            let a = walk(then_branch, &mut left, facts);
            let b = walk(else_branch, &mut right, facts);
            scope.locals = left
                .locals
                .into_iter()
                .filter(|(n, s)| right.locals.get(n) == Some(s))
                .collect();
            let before = scope.returns.len();
            scope.returns.extend(left.returns.into_iter().skip(before));
            scope.returns.extend(right.returns.into_iter().skip(before));
            if a == b { a } else { Side::Unknown }
        }
        ExprNode::Send {
            recv,
            method,
            args,
            block,
            ..
        } => {
            let side = recv
                .as_mut()
                .map(|r| walk(r, scope, facts))
                .unwrap_or(scope.self_side);
            let no_args = args.is_empty();
            for arg in args {
                walk(arg, scope, facts);
            }
            if let Some(block) = block {
                forget_writes(block, scope);
                let mut inner = scope.clone();
                // Arbitrary app blocks can rebind self; only ordinary
                // framework record iterators grant a yielded instance.
                inner.self_side = Side::Unknown;
                if let ExprNode::Lambda {
                    params,
                    rest_param,
                    extra_params,
                    block_param,
                    body,
                    ..
                } = &mut *block.node
                {
                    for p in params
                        .iter()
                        .chain(rest_param.iter())
                        .chain(block_param.iter())
                        .chain(extra_params.iter().map(|p| &p.name))
                    {
                        inner.locals.remove(p);
                    }
                    if let Some(r) = recv.as_ref()
                        && ITERATORS.contains(&method.as_str())
                        && record_source(facts, r)
                        && !overridden(facts, r, true, method)
                        && let Some(name) = params.first()
                    {
                        inner.locals.insert(name.clone(), Side::Instance);
                    }
                    walk(body, &mut inner, facts);
                } else {
                    walk(block, &mut inner, facts);
                }
            }
            if let Some(r) = recv.as_ref() {
                if method.as_str() == "class"
                    && no_args
                    && side != Side::Unknown
                    && !overridden(facts, r, side == Side::Class, method)
                {
                    Side::Class
                } else if RECORDS.contains(&method.as_str())
                    && record_source(facts, r)
                    && !overridden(facts, r, true, method)
                {
                    Side::Instance
                } else if side != Side::Unknown
                    && owner(r).is_some_and(|id| {
                        facts.class_returns.contains(&(
                            id.clone(),
                            side == Side::Class,
                            method.clone(),
                        ))
                    })
                {
                    Side::Class
                } else {
                    Side::Unknown
                }
            } else if side == Side::Class
                && RECORDS.contains(&method.as_str())
                && scope.owner.as_ref().is_some_and(|id| {
                    facts.models.contains(id)
                        && !facts
                            .overrides
                            .contains(&(id.clone(), true, method.clone()))
                })
            {
                Side::Instance
            } else if side != Side::Unknown
                && scope.owner.as_ref().is_some_and(|id| {
                    facts
                        .class_returns
                        .contains(&(id.clone(), side == Side::Class, method.clone()))
                })
            {
                Side::Class
            } else {
                Side::Unknown
            }
        }
        ExprNode::Lambda {
            params,
            rest_param,
            extra_params,
            block_param,
            body,
            ..
        } => {
            forget_writes(body, scope);
            let mut inner = scope.clone();
            for p in params
                .iter()
                .chain(rest_param.iter())
                .chain(block_param.iter())
                .chain(extra_params.iter().map(|p| &p.name))
            {
                inner.locals.remove(p);
            }
            walk(body, &mut inner, facts);
            Side::Unknown
        }
        _ => {
            // Loops, rescue joins, destructuring and other unmodeled writes
            // cannot retain a previous local's instance proof.
            forget_writes(expr, scope);
            let before = scope.returns.len();
            let mut returns = Vec::new();
            expr.node.for_each_child_mut(&mut |e| {
                let mut inner = scope.clone();
                walk(e, &mut inner, facts);
                returns.extend(inner.returns.into_iter().skip(before));
            });
            scope.returns.extend(returns);
            Side::Unknown
        }
    };
    // An existing exact class fact wins over lexical/nominal evidence,
    // including self bindings and class-valued locals already analyzed.
    let side = if expr.decisions & CLASS_OBJECT_VALUE != 0 {
        Side::Class
    } else {
        side
    };
    expr.decisions &= !(INDEXER_INSTANCE_VALUE | INDEXER_CLASS_QUERY_VALUE);
    expr.decisions |= match side {
        Side::Instance => INDEXER_INSTANCE_VALUE,
        Side::Class => INDEXER_CLASS_QUERY_VALUE,
        Side::Unknown => 0,
    };
    side
}
