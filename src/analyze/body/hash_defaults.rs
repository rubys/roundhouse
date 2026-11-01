//! Nil-default evidence for fresh hashes and unescaped local aliases.

use crate::det::{HashMap, HashSet};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;

#[derive(Default)]
pub(super) struct NilDefaults {
    proven: HashSet<Symbol>,
}

impl NilDefaults {
    pub(super) fn of(body: &Expr, ctx: &super::Ctx) -> Self {
        #[derive(Default)]
        struct Scan {
            counts: HashMap<Symbol, usize>,
            literals: HashSet<Symbol>,
            aliases: HashMap<Symbol, Vec<Symbol>>,
            escaped: HashSet<Symbol>,
            dynamic: bool,
        }
        fn bind(scan: &mut Scan, name: &Symbol, value: &Expr) {
            match &*value.node {
                ExprNode::Hash { .. } => {
                    scan.literals.insert(name.clone());
                }
                ExprNode::Var { name: source, .. } => {
                    scan.aliases
                        .entry(name.clone())
                        .or_default()
                        .push(source.clone());
                }
                _ => {}
            }
        }
        fn visit(e: &Expr, scan: &mut Scan, allowed_local: bool) {
            let mut names = Vec::new();
            super::rebound_names(e, &mut names);
            for name in names {
                *scan.counts.entry(name).or_default() += 1;
            }
            match &*e.node {
                ExprNode::Var { name, .. } => {
                    if !allowed_local {
                        scan.escaped.insert(name.clone());
                    }
                }
                ExprNode::Assign {
                    target: LValue::Var { name, .. },
                    value,
                } => {
                    bind(scan, name, value);
                    visit(value, scan, matches!(&*value.node, ExprNode::Var { .. }));
                    return;
                }
                ExprNode::Let {
                    name, value, body, ..
                } => {
                    bind(scan, name, value);
                    visit(value, scan, matches!(&*value.node, ExprNode::Var { .. }));
                    visit(body, scan, false);
                    return;
                }
                ExprNode::Send {
                    recv,
                    method,
                    args,
                    block,
                    ..
                } => {
                    scan.dynamic |= matches!(
                        method.as_str(),
                        "binding"
                            | "eval"
                            | "local_variable_set"
                            | "instance_eval"
                            | "class_eval"
                            | "module_eval"
                    );
                    let read = method.as_str() == "dig"
                        || matches!(method.as_str(), "send" | "public_send" | "__send__")
                            && args.first().is_some_and(|a| match &*a.node {
                                ExprNode::Lit {
                                    value: crate::expr::Literal::Sym { value },
                                } => value.as_str() == "dig",
                                ExprNode::Lit {
                                    value: crate::expr::Literal::Str { value },
                                } => value.as_str() == "dig",
                                _ => false,
                            });
                    if let Some(receiver) = recv {
                        visit(receiver, scan, read);
                    }
                    for arg in args {
                        visit(arg, scan, false);
                    }
                    if let Some(block) = block {
                        visit(block, scan, false);
                    }
                    return;
                }
                _ => {}
            }
            e.node
                .for_each_child(&mut |child| visit(child, scan, false));
        }
        fn origin(name: &Symbol, scan: &Scan) -> Option<Symbol> {
            let mut current = name;
            let mut seen = HashSet::default();
            while seen.insert(current) && scan.counts.get(current) == Some(&1) {
                if scan.literals.contains(current) {
                    return Some(current.clone());
                }
                let aliases = scan.aliases.get(current)?;
                if aliases.len() != 1 {
                    return None;
                }
                current = &aliases[0];
            }
            None
        }
        let mut scan = Scan::default();
        // Incoming parameters/captures are not fresh local hash bindings.
        scan.counts
            .extend(ctx.local_bindings.keys().cloned().map(|name| (name, 1)));
        visit(body, &mut scan, false);
        if scan.dynamic {
            return Self::default();
        }
        // Even a rebound alias may have mutated an earlier referent. Propagate
        // escapes through every possible alias edge, not just proven bindings.
        let mut escaped = scan.escaped.clone();
        loop {
            let before = escaped.len();
            for (name, sources) in &scan.aliases {
                if escaped.contains(name) || sources.iter().any(|source| escaped.contains(source)) {
                    escaped.insert(name.clone());
                    escaped.extend(sources.iter().cloned());
                }
            }
            if escaped.len() == before {
                break;
            }
        }
        let proven = scan
            .counts
            .keys()
            .filter(|name| origin(name, &scan).is_some_and(|root| !escaped.contains(&root)))
            .cloned()
            .collect();
        Self { proven }
    }

    pub(super) fn proves(&self, receiver: &Expr) -> bool {
        match &*receiver.node {
            ExprNode::Hash { .. } => true,
            ExprNode::Var { name, .. } => self.proven.contains(name),
            _ => false,
        }
    }
}
