//! A class method an app writes to define methods, expanded at each
//! class-body call into the methods it defines.
//!
//! forem's `Settings::Base.setting` (adapted from rails-settings-cached)
//! is the shape: a subclass writes `setting :community_name, type:
//! :string, default: …`, the inherited class method hands its arguments
//! to a private `define_setting`, and that one calls
//! `define_singleton_method(key) { … }` for a reader and writers whose
//! bodies read the captured arguments. Nothing in the source declares
//! `Settings::Community.community_name`; Ruby makes it when the class
//! body runs.
//!
//! The macro's body runs here, partially: its arguments are bound, the
//! statements that only compute from them (`key = key.to_s`, `type ||
//! :string`, a guard that does not fire) are evaluated, a call to
//! another of the class's methods is followed into, and each
//! `define_method` / `define_singleton_method` with a name that comes
//! out literal becomes an ordinary `def` in the calling class. The
//! defined bodies read the captured values as Ruby's closures do: a
//! Symbol, number, `true`, `false` or `nil` as that literal, anything
//! else (a String, `Time.zone.today.year`) through a constant the
//! expansion assigns where the call is, so it is evaluated once, then,
//! and shared by every method that captures it. A statement on the
//! class's own state (`@defined_settings << {…}`) runs in the calling
//! class's body, as it does when the macro runs there.
//!
//! What cannot be evaluated this way — a guard on a value known only at
//! run time, a name computed from one, a captured local the defined body
//! reassigns, a call the expansion cannot follow — leaves the call as it
//! was and reports it, rather than dropping the methods it would have
//! defined.

use crate::det::{HashMap, HashSet};

use crate::App;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, MethodVisibility, ModelBodyItem, Param};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::{SourceFile, Span};

use crate::ingest::prism_api::PrismLocation;
const MAX_DEPTH: usize = 8;

pub(crate) fn expand_class_macros(app: &mut App, sources: &[SourceFile], mention_counts: &Mentions<'_>) {
    let parents: HashMap<ClassId, ClassId> = app
        .models
        .iter()
        .filter_map(|m| Some((m.name.clone(), m.parent.clone()?)))
        .chain(app.library_classes.iter().filter(|lc| !lc.is_module).filter_map(|lc| Some((lc.name.clone(), lc.parent.clone()?))))
        .collect();
    let named = Named::of(app);
    let hooks = hook_macro_table(app);
    let cvar_readers: HashMap<(String, String), Symbol> = app
        .library_classes
        .iter()
        .flat_map(|c| {
            c.methods.iter().filter(|m| m.receiver == MethodReceiver::Class && m.params.is_empty()).filter_map(move |m| {
                let body = match &*m.body.node {
                    ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
                    _ => &m.body,
                };
                match &*body.node {
                    ExprNode::Var { name, .. } if name.as_str().starts_with("@@") => {
                        Some(((c.name.0.as_str().to_string(), name.as_str().to_string()), m.name.clone()))
                    }
                    _ => None,
                }
            })
        })
        .collect();
    let mut module_methods: HashMap<String, Vec<MethodDef>> = HashMap::default();
    for lc in app.library_classes.iter().filter(|lc| lc.is_module) {
        module_methods.entry(lc.name.0.as_str().to_string()).or_default().extend(lc.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).cloned());
    }
    let extended_from: HashMap<Symbol, ClassId> = app
        .load_hook_extended_macros
        .iter()
        .flat_map(|id| {
            app.library_classes.iter().filter(move |c| &c.name == id).flat_map(move |c| {
                c.methods.iter().filter(|m| m.receiver == MethodReceiver::Instance).map(move |m| (m.name.clone(), id.clone()))
            })
        })
        .collect();
    let frozen = frozen_readers(app, sources, mention_counts);
    let known: HashSet<String> = app
        .library_classes
        .iter()
        .map(|c| c.name.0.as_str().to_string())
        .chain(app.models.iter().map(|m| m.name.0.as_str().to_string()))
        .collect();
    for model_index in 0..app.models.len() {
        let ancestors = ancestors(&named, &parents, &app.models[model_index].name);
        let mut table = class_method_table(app, &named, &ancestors);
        // Not the model's own: the `on_load` reopen of ActiveRecord::Base carries the hook's very definitions (audited's Concern).
        let own_names: HashSet<Symbol> =
            table.iter().filter(|(name, def)| hooks.get(*name).is_none_or(|hook| hook.name_span != def.name_span)).map(|(name, _)| name.clone()).collect();
        for (name, def) in &hooks {
            table.entry(name.clone()).or_insert_with(|| def.clone());
        }
        if table.is_empty() {
            continue;
        }
        let macros: HashSet<Symbol> = table
            .keys()
            .filter(|name| (hooks.contains_key(*name) && !own_names.contains(*name)) || reaches_definition(name, &table, 0))
            .cloned()
            .collect();
        if macros.is_empty() {
            continue;
        }
        let constants = literal_constants(app, &ancestors);
        let model = &app.models[model_index];
        // Not one this can follow: a class that redefines the definers, or a macro also called where the class body does not always run it.
        let redefined = table.contains_key(&Symbol::from("define_method")) || table.contains_key(&Symbol::from("define_singleton_method"));
        let mut nested: HashSet<Symbol> = HashSet::default();
        for item in &model.body {
            let ModelBodyItem::Unknown { expr, .. } = item else { continue };
            let root = match &*expr.node {
                ExprNode::Send { method, .. } => Some(method),
                _ => None,
            };
            let mut first = true;
            visit(expr, &mut |e| {
                if let ExprNode::Send { recv, method, .. } = &*e.node
                    && macros.contains(method)
                    && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                    && !(first && root == Some(method))
                {
                    nested.insert(method.clone());
                }
                first = false;
            });
        }
        let own: HashSet<(Symbol, MethodReceiver)> = model.methods().map(|m| (m.name.clone(), m.receiver)).collect();
        let prefix = slot_prefix(model.name.0.as_str());
        let mut counter = 0usize;
        let mut defined: HashSet<(Symbol, MethodReceiver)> = HashSet::default();
        let mut reserved: Option<HashSet<Symbol>> = None;
        let mut expansions: HashMap<usize, Vec<ModelBodyItem>> = HashMap::default();
        let mut defaults: indexmap::IndexMap<Symbol, Expr> = indexmap::IndexMap::new();
        let mut extended: HashSet<ClassId> = HashSet::default();
        let mut included: Vec<String> = app.load_hook_class_macros.iter().map(|id| id.0.as_str().to_string()).collect();
        for id in ancestors.iter().chain(std::iter::once(&model.name)) {
            for item in app.models.iter().filter(|m| &m.name == id).flat_map(|m| m.body.iter()) {
                if let ModelBodyItem::Unknown { expr, .. } = item
                    && let ExprNode::Send { recv: None, method, args, .. } = &*expr.node
                    && method.as_str() == "include"
                {
                    included.extend(args.iter().filter_map(|a| match &*a.node {
                        ExprNode::Const { path } => Some(path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::")),
                        _ => None,
                    }));
                }
            }
            for lc in app.library_classes.iter().filter(|lc| &lc.name == id) {
                included.extend(lc.includes.iter().map(|i| i.0.as_str().trim_start_matches("::").to_string()));
            }
        }
        for (index, item) in model.body.iter().enumerate() {
            let ModelBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv, method, args, block, .. } = &*expr.node else { continue };
            if !macros.contains(method) || recv.as_ref().is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef)) {
                continue;
            }
            let mut run = Run { table: &table, constants: &constants, frozen: &frozen, cvar_readers: &cvar_readers, included: included.clone(), module_methods: &module_methods, extended: HashMap::default(), evaluated: Vec::new(), known: &known, scope: None, prefix: &prefix, counter, out: Vec::new(), replayed: HashSet::default(), class_attrs: HashMap::default(), unset_cattrs: HashMap::default(), methods: Vec::new(), aliases: Vec::new(), strings: HashMap::default(), owner: model.name.0.clone(), sources, instance_methods: None };
            let result = if block.as_ref().is_some_and(|b| !matches!(&*b.node, ExprNode::Lambda { .. })) {
                Err("it is passed a block other than a literal one".to_string())
            } else if redefined {
                Err("the class redefines `define_method`".to_string())
            } else if nested.contains(method) {
                Err("it is also called where the class body may not run it".to_string())
            } else if !call_syntax_kept(expr, sources) {
                Err("its arguments use a splat or a block argument".to_string())
            } else {
                run.call(method, args, block.as_ref(), &HashMap::default(), 0)
            }
            .and_then(|()| {
                for m in &run.methods {
                    let key = (m.name.clone(), m.receiver);
                    if own.contains(&key) || !defined.insert(key) {
                        return Err(format!("`{}` is defined twice", m.name.as_str()));
                    }
                    if m.receiver == MethodReceiver::Instance && reserved.get_or_insert_with(|| synthesized_names(app, model_index)).contains(&m.name) {
                        return Err(format!("`{}` is also a method the model's associations or declarations define", m.name.as_str()));
                    }
                }
                Ok(())
            });
            match result {
                Ok(()) => {
                    counter = run.counter;
                    let mut items: Vec<ModelBodyItem> = Vec::new();
                    // Not left to the hook: Rails extends every model with the module, so the macro's class-side readers answer here too.
                    if !own_names.contains(method)
                        && let Some(module) = extended_from.get(method)
                        && extended.insert(module.clone())
                    {
                        let path = std::iter::once(Symbol::from("")).chain(module.0.as_str().split("::").map(Symbol::from)).collect();
                        let extend = Expr::new(expr.span, ExprNode::Send {
                            recv: None,
                            method: Symbol::from("extend"),
                            args: vec![Expr::new(expr.span, ExprNode::Const { path })],
                            block: None,
                            parenthesized: false,
                        });
                        items.extend(replay(&extend, &model.name, &mut defaults));
                    }
                    for (index, mut statement) in run.out.into_iter().enumerate() {
                        if run.replayed.contains(&index) {
                            items.extend(replay(&statement, &model.name, &mut defaults));
                        } else {
                            // At the call, not the gem's line: the model's body orders its statements by offset.
                            if !matches!(&*statement.node, ExprNode::Assign { target: LValue::Const { .. }, .. }) {
                                statement.span = expr.span;
                            }
                            items.push(ModelBodyItem::Unknown { expr: statement, leading_comments: Vec::new(), leading_blank_line: false });
                        }
                    }
                    items.extend(run.methods.into_iter().map(|method| ModelBodyItem::Method { method, leading_comments: Vec::new(), leading_blank_line: false }));
                    for code in &run.evaluated {
                        items.extend(replay_source(code, &model.name, &mut defaults));
                    }
                    items.extend(run.aliases.into_iter().map(|(new, old, span)| ModelBodyItem::Alias { new, old, leading_comments: Vec::new(), leading_blank_line: false, span }));
                    expansions.insert(index, items);
                }
                Err(why) => {
                    super::prism::push(crate::diagnostic::Diagnostic::unsupported(
                        expr.span,
                        None,
                        "class macro",
                        format!("`{}` defines methods this call does not expand ({why}), so they are missing", method.as_str()),
                    ));
                }
            }
        }
        if expansions.is_empty() {
            continue;
        }
        let model = &mut app.models[model_index];
        model.class_attr_defaults.extend(defaults);
        let mut body = Vec::with_capacity(model.body.len());
        for (index, item) in std::mem::take(&mut model.body).into_iter().enumerate() {
            match expansions.remove(&index) {
                Some(items) => body.extend(items),
                None => body.push(item),
            }
        }
        model.body = body;
    }
    // A plain class's body call to its own (or an ancestor's) `def self.` macro
    // runs as the class loads, as a model's does (#1281). Replayed instead, a
    // `define_method` with a computed name is one the Spinel target refuses.
    // Only a run that just defines methods expands; a call that can't is
    // reported once, here, and not replayed.
    let mut expanded_total: HashMap<Symbol, usize> = HashMap::default();
    let mut failed_total: HashSet<Symbol> = HashSet::default();
    for lc_index in 0..app.library_classes.len() {
        let lc = &app.library_classes[lc_index];
        if lc.is_module || lc.origin.is_some() || lc.unknown_calls.is_empty() {
            continue;
        }
        let mut lineage = vec![lc.name.clone()];
        lineage.extend(ancestors(&named, &parents, &lc.name));
        let table = class_method_table(app, &named, &lineage);
        let macros: HashSet<Symbol> = table.keys().filter(|name| reaches_definition(name, &table, 0)).cloned().collect();
        if macros.is_empty() {
            continue;
        }
        let constants = literal_constants(app, &lineage);
        let redefined = table.contains_key(&Symbol::from("define_method")) || table.contains_key(&Symbol::from("define_singleton_method"));
        let own: HashSet<(Symbol, MethodReceiver)> = lc.methods.iter().map(|m| (m.name.clone(), m.receiver)).collect();
        let prefix = slot_prefix(lc.name.0.as_str());
        let included: Vec<String> = lineage
            .iter()
            .flat_map(|id| app.library_classes.iter().filter(move |c| &c.name == id))
            .flat_map(|c| c.includes.iter().map(|i| i.0.as_str().trim_start_matches("::").to_string()))
            .collect();
        // Not answered past a parent this program does not define: its methods are not known here.
        let known_root = lineage.last().is_none_or(|top| {
            app.library_classes.iter().filter(|c| &c.name == top).all(|c| {
                c.parent.as_ref().is_none_or(|p| app.library_classes.iter().any(|o| &o.name == p) || app.models.iter().any(|m| &m.name == p))
            })
        });
        // Not a private one: `method_defined?` answers public and protected methods only.
        let instance_methods: Option<HashSet<Symbol>> = known_root.then(|| {
            lineage
                .iter()
                .flat_map(|id| app.library_classes.iter().filter(move |c| &c.name == id))
                .flat_map(|c| c.methods.iter())
                .chain(included.iter().flat_map(|full| module_methods.get(full).into_iter().flatten()))
                .filter(|m| m.receiver == MethodReceiver::Instance && m.visibility != MethodVisibility::Private)
                .map(|m| m.name.clone())
                .collect()
        });
        let mut slot_constants: Vec<(Symbol, Expr)> = Vec::new();
        let mut counter = 0usize;
        let mut defined: HashSet<(Symbol, MethodReceiver)> = HashSet::default();
        let mut added: Vec<MethodDef> = Vec::new();
        let mut consumed: HashSet<usize> = HashSet::default();
        let mut expanded: HashMap<Symbol, usize> = HashMap::default();
        let mut failed: HashSet<Symbol> = HashSet::default();
        for (index, expr) in lc.unknown_calls.iter().enumerate() {
            let ExprNode::Send { recv, method, args, block, .. } = &*expr.node else { continue };
            if !macros.contains(method) || recv.as_ref().is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef)) {
                continue;
            }
            let mut run = Run { table: &table, constants: &constants, frozen: &frozen, cvar_readers: &cvar_readers, included: included.clone(), module_methods: &module_methods, extended: HashMap::default(), evaluated: Vec::new(), known: &known, scope: None, prefix: &prefix, counter, out: Vec::new(), replayed: HashSet::default(), class_attrs: HashMap::default(), unset_cattrs: HashMap::default(), methods: Vec::new(), aliases: Vec::new(), strings: HashMap::default(), owner: lc.name.0.clone(), sources, instance_methods: instance_methods.as_ref() };
            let result = if block.as_ref().is_some_and(|b| !matches!(&*b.node, ExprNode::Lambda { .. })) {
                Err("it is passed a block other than a literal one".to_string())
            } else if redefined {
                Err("the class redefines `define_method`".to_string())
            } else if !call_syntax_kept(expr, sources) {
                Err("its arguments use a splat or a block argument".to_string())
            } else {
                run.call(method, args, block.as_ref(), &HashMap::default(), 0)
            }
            .and_then(|()| {
                // A value the defined bodies capture is the class's constant (`precedence BASE` -> `ROUNDHOUSE_MACRO_…_1 = BASE`).
                let slots: Vec<(Symbol, Expr)> = run
                    .out
                    .iter()
                    .filter_map(|st| match &*st.node {
                        ExprNode::Assign { target: LValue::Const { path }, value } if path.len() == 1 && path[0].as_str().starts_with(prefix.as_str()) => {
                            Some((path[0].clone(), value.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                // Not a plain class's to carry: what a model's expansion places in its body.
                if slots.len() != run.out.len() || !run.evaluated.is_empty() || !run.aliases.is_empty() || !run.extended.is_empty() || !run.class_attrs.is_empty() {
                    return Err("it does more than define methods".to_string());
                }
                for m in &run.methods {
                    let key = (m.name.clone(), m.receiver);
                    if own.contains(&key) || !defined.insert(key) {
                        return Err(format!("`{}` is defined twice", m.name.as_str()));
                    }
                }
                slot_constants.extend(slots);
                Ok(())
            });
            consumed.insert(index);
            match result {
                Ok(()) => {
                    counter = run.counter;
                    added.extend(run.methods);
                    *expanded.entry(method.clone()).or_default() += 1;
                }
                Err(why) => {
                    failed.insert(method.clone());
                    super::prism::push(crate::diagnostic::Diagnostic::unsupported(
                        expr.span,
                        None,
                        "class macro",
                        format!("`{}` defines methods this call does not expand ({why}), so they are missing", method.as_str()),
                    ));
                }
            }
        }
        for (name, calls) in &expanded {
            *expanded_total.entry(name.clone()).or_default() += calls;
        }
        failed_total.extend(failed.iter().cloned());
        if consumed.is_empty() {
            continue;
        }
        // The class's own macro goes too once every mention of it is a call that expanded: its body
        // (`define_method(meth, …)` over a name known only when it runs) is what a target refuses.
        let spent: HashSet<Symbol> = expanded
            .iter()
            .filter(|(name, calls)| !failed.contains(*name) && mention_counts.count(name.as_str()) == **calls)
            .map(|(name, _)| name.clone())
            .collect();
        let lc = &mut app.library_classes[lc_index];
        lc.methods.retain(|m| !(m.receiver == MethodReceiver::Class && spent.contains(&m.name)));
        lc.methods.extend(added);
        lc.constants.extend(slot_constants);
        let calls = std::mem::take(&mut lc.unknown_calls);
        lc.unknown_calls = calls.into_iter().enumerate().filter(|(i, _)| !consumed.contains(i)).map(|(_, e)| e).collect();
    }
    // So does an ancestor's (`class Phrases < RuleBase; rule(:word) { … }`), once every call anywhere expanded.
    // Counted on the IR, not the text: a comment that says "precedence" is no call, and neither is an
    // instance method's own receiverless `precedence`. What is left is a call by name the expansion did
    // not consume — `Base.precedence(…)`, a class method's `precedence(…)`, or `:precedence` handed to `send`.
    // A receiver counts when it is a class that has the macro (it or an ancestor defines it), not `Rails.root`.
    let owners = |name: &Symbol| -> HashSet<String> {
        let defining: Vec<&ClassId> = app
            .library_classes
            .iter()
            .filter(|lc| lc.methods.iter().any(|m| m.receiver == MethodReceiver::Class && &m.name == name))
            .map(|lc| &lc.name)
            .collect();
        app.library_classes
            .iter()
            .filter(|lc| ancestors(&named, &parents, &lc.name).iter().chain(std::iter::once(&lc.name)).any(|a| defining.contains(&a)))
            .map(|lc| lc.name.0.as_str().to_string())
            .collect()
    };
    fn class_side_mentions(e: &Expr, name: &Symbol, class_body: bool, owners: &HashSet<String>) -> usize {
        let on_owner = |recv: Option<&Expr>| match recv.map(|r| &*r.node) {
            Some(ExprNode::Const { path }) => {
                let full = path.iter().map(|p| p.as_str()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("::");
                owners.iter().any(|o| o == &full || o.ends_with(&format!("::{full}"))) as usize
            }
            Some(ExprNode::SelfRef) | None => class_body as usize,
            _ => 0,
        };
        let here = match &*e.node {
            ExprNode::Send { recv, method, .. } if method == name => on_owner(recv.as_ref()),
            // `send(:root)`, `method(:root)`: a call by a name held as a Symbol, on the macro's owner.
            ExprNode::Send { recv, method, args, .. }
                if matches!(method.as_str(), "send" | "public_send" | "__send__" | "method" | "public_method" | "respond_to?")
                    && matches!(args.first().map(|a| &*a.node), Some(ExprNode::Lit { value: Literal::Sym { value } }) if value == name) =>
            {
                on_owner(recv.as_ref())
            }
            // `&method(:level)`: ingest's bound-method reference.
            ExprNode::MethodRef { recv, name: referenced } if referenced == name => on_owner(recv.as_ref()),
            _ => 0,
        };
        let mut inner = 0;
        e.node.for_each_child(&mut |c| inner += class_side_mentions(c, name, class_body, owners));
        here + inner
    }
    let remaining = |name: &Symbol| -> usize {
        let owners = owners(name);
        let mut everywhere = 0;
        crate::lower::for_each_emit_body_ref(app, &mut |e| everywhere += class_side_mentions(e, name, false, &owners));
        let class_bodies: Vec<&MethodDef> = app
            .library_classes
            .iter()
            .flat_map(|lc| lc.methods.iter())
            .filter(|m| m.receiver == MethodReceiver::Class && &m.name != name)
            .collect();
        // Not every call in a same-named class method: a subclass's `def self.level(v) = RuleBase.level(v)` still reaches the macro.
        let own: usize = app
            .library_classes
            .iter()
            .flat_map(|lc| {
                let only: HashSet<String> = std::iter::once(lc.name.0.to_string()).collect();
                lc.methods
                    .iter()
                    .filter(|m| m.receiver == MethodReceiver::Class && &m.name == name)
                    .map(move |m| class_side_mentions(&m.body, name, false, &only))
                    .collect::<Vec<_>>()
            })
            .sum();
        let class_side: usize = class_bodies
            .iter()
            .map(|m| class_side_mentions(&m.body, name, true, &owners) - class_side_mentions(&m.body, name, false, &owners))
            .sum();
        everywhere.saturating_sub(own) + class_side
    };
    let spent: HashSet<Symbol> = expanded_total
        .keys()
        .filter(|name| !failed_total.contains(*name) && remaining(name) == 0)
        .cloned()
        .collect();
    if !spent.is_empty() {
        for lc in app.library_classes.iter_mut().filter(|lc| !lc.is_module) {
            lc.methods.retain(|m| !(m.receiver == MethodReceiver::Class && spent.contains(&m.name)));
        }
    }
}

pub(crate) fn hook_macro_calls(app: &App) -> HashSet<Symbol> {
    let hooks = hook_macro_table(app);
    let mut called = HashSet::default();
    for model in &app.models {
        for item in &model.body {
            let ModelBodyItem::Unknown { expr, .. } = item else { continue };
            if let ExprNode::Send { recv, method, .. } = &*expr.node
                && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                && hooks.contains_key(method)
            {
                called.insert(method.clone());
            }
        }
    }
    called
}

/// Not kept once expanded: analyzed on the gem's module, the macro's `has_many` and `include` are reported.
pub(crate) fn drop_expanded_hook_macros(app: &mut App, expanded: &HashSet<Symbol>, mention_counts: &Mentions<'_>) {
    if expanded.is_empty() {
        return;
    }
    // The `on_load` reopen of ActiveRecord::Base carries an included hook module's class methods too
    let origins: Vec<(ClassId, MethodReceiver)> = app
        .load_hook_class_macros
        .iter()
        .map(|id| (id.clone(), MethodReceiver::Class))
        .chain(app.load_hook_extended_macros.iter().map(|id| (id.clone(), MethodReceiver::Instance)))
        .chain(std::iter::once((ClassId(Symbol::from("ActiveRecord::Base")), MethodReceiver::Class)))
        .collect();
    let hooked = |class: &ClassId, m: &MethodDef| origins.iter().any(|(id, receiver)| id == class && *receiver == m.receiver);
    // Not kept for a caller nothing calls: ancestry's `acts_as_tree` only forwards to the expanded `has_ancestry`.
    let dead: HashSet<Symbol> = app
        .library_classes
        .iter()
        .flat_map(|c| c.methods.iter().filter(|m| hooked(&c.name, m)))
        .filter(|m| !expanded.contains(&m.name) && !implicitly_called(m.name.as_str()) && !mention_counts.mentioned(m.name.as_str()))
        .map(|m| m.name.clone())
        .collect();
    let mut called: HashSet<Symbol> = HashSet::default();
    let mut note = |e: &Expr| {
        visit(e, &mut |x| {
            if let ExprNode::Send { method, .. } = &*x.node
                && expanded.contains(method)
            {
                called.insert(method.clone());
            }
        })
    };
    for model in &app.models {
        for item in &model.body {
            if let ModelBodyItem::Unknown { expr, .. } = item {
                note(expr);
            }
        }
        for m in model.methods() {
            note(&m.body);
        }
    }
    for class in &app.library_classes {
        for call in &class.unknown_calls {
            note(call);
        }
        for m in &class.methods {
            if !expanded.contains(&m.name) && !(hooked(&class.name, m) && dead.contains(&m.name)) {
                note(&m.body);
            }
        }
    }
    for class in app.library_classes.iter_mut() {
        let name = class.name.clone();
        class.methods.retain(|m| {
            !origins.iter().any(|(id, receiver)| id == &name && *receiver == m.receiver)
                || (!dead.contains(&m.name) && (!expanded.contains(&m.name) || called.contains(&m.name)))
        });
    }
}

/// The macros a gem's `ActiveSupport.on_load(:active_record)` gives
/// every model: an included module's class methods, an extended
/// module's instance methods, each as a class method of the model.
fn hook_macro_table(app: &App) -> HashMap<Symbol, MethodDef> {
    let mut table = HashMap::default();
    let sources = app
        .load_hook_class_macros
        .iter()
        .map(|id| (id, MethodReceiver::Class))
        .chain(app.load_hook_extended_macros.iter().map(|id| (id, MethodReceiver::Instance)));
    for (id, receiver) in sources {
        for class in app.library_classes.iter().filter(|c| &c.name == id) {
            for m in class.methods.iter().filter(|m| m.receiver == receiver) {
                let mut def = m.clone();
                def.receiver = MethodReceiver::Class;
                table.entry(def.name.clone()).or_insert(def);
            }
        }
    }
    table
}

/// `Owner.reader` whose body reads a class variable the class body
/// initializes to a literal, and which nothing in the program writes
/// again: ancestry's `Ancestry.default_ancestry_format`. Its setter
/// exists, so a call to it anywhere (or a `class_variable_set` naming
/// the variable) keeps the reader a value known only when it runs.
fn frozen_readers(app: &App, sources: &[SourceFile], mention_counts: &Mentions<'_>) -> HashMap<(String, Symbol), Expr> {
    let mut out = HashMap::default();
    // Not computed up front: it indexes every source, and most apps have no frozen reader to ask about.
    let dead: std::cell::OnceCell<Vec<(usize, usize, usize)>> = std::cell::OnceCell::new();
    for class in &app.library_classes {
        let initial: HashMap<&str, &Expr> = class
            .class_ivar_initializers
            .iter()
            .filter_map(|e| match &*e.node {
                ExprNode::Assign { target: LValue::Var { name, .. }, value }
                    if name.as_str().starts_with("@@") && matches!(&*value.node, ExprNode::Lit { .. }) =>
                {
                    Some((name.as_str(), value))
                }
                _ => None,
            })
            .collect();
        if initial.is_empty() {
            continue;
        }
        // Not only `def self.` readers: a module's reader answers on whoever `extend`s it (rolify's `Rolify.orm`).
        let owners: Vec<(&str, MethodReceiver)> = std::iter::once((class.name.0.as_str(), MethodReceiver::Class))
            .chain(extenders(app, class).into_iter().map(|owner| (owner, MethodReceiver::Instance)))
            .collect();
        for (owner, receiver) in owners {
            for reader in class.methods.iter().filter(|m| m.receiver == receiver && m.params.is_empty()) {
                let body = match &*reader.body.node {
                    ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
                    _ => &reader.body,
                };
                let ExprNode::Var { name: cvar, .. } = &*body.node else { continue };
                let Some(value) = initial.get(cvar.as_str()) else { continue };
                let writers: Vec<&MethodDef> = class.methods.iter().filter(|m| assigns(&m.body, cvar)).collect();
                if writers.iter().any(|w| !w.name.as_str().ends_with('=')) {
                    continue;
                }
                let setters: Vec<&str> = writers.iter().map(|w| w.name.as_str().trim_end_matches('=')).collect();
                let dead = dead.get_or_init(|| uncalled_bodies(app, mention_counts));
                if sources.iter().enumerate().any(|(i, source)| writes_through(&source.text, &setters, cvar.as_str(), &|at| dead.iter().any(|&(f, a, b)| f == i && a <= at && at < b))) {
                    continue;
                }
                out.insert((owner.to_string(), reader.name.clone()), (*value).clone());
            }
        }
    }
    out
}

fn extenders<'a>(app: &'a App, module: &LibraryClass) -> Vec<&'a str> {
    if !module.is_module {
        return Vec::new();
    }
    let name = module.name.0.as_str();
    app.library_classes
        .iter()
        .filter(|owner| {
            owner.unknown_calls.iter().any(|call| match &*call.node {
                ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "extend" => args.iter().any(|a| match &*a.node {
                    ExprNode::Const { path } => {
                        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                        written == name || format!("{}::{written}", owner.name.0.as_str()) == name
                    }
                    _ => false,
                }),
                _ => false,
            })
        })
        .map(|owner| owner.name.0.as_str())
        .collect()
}

// Not every write counts: one in a method nothing calls never runs (rolify's `use_dynamic_shortcuts`).
fn uncalled_bodies(app: &App, mention_counts: &Mentions<'_>) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for class in &app.library_classes {
        for m in class.methods.iter().filter(|m| !mention_counts.mentioned(m.name.as_str())) {
            // Not the body's own span: a multi-statement body is a synthetic Seq.
            let mut range: Option<(crate::span::FileId, u32, u32)> = None;
            visit(&m.body, &mut |e| {
                if e.span.is_synthetic() {
                    return;
                }
                range = Some(match range {
                    Some((file, a, b)) if file == e.span.file => (file, a.min(e.span.start), b.max(e.span.end)),
                    Some(other) => other,
                    None => (e.span.file, e.span.start, e.span.end),
                });
            });
            if let Some((file, a, b)) = range
                && let Some(i) = file.0.checked_sub(1)
            {
                out.push((i as usize, a as usize, b as usize));
            }
        }
    }
    out
}

// Not "unmentioned, so dead": Ruby and Rails call these by protocol, with no call in any source.
fn implicitly_called(name: &str) -> bool {
    matches!(
        name,
        "inherited" | "included" | "extended" | "prepended" | "method_added" | "method_missing" | "respond_to_missing?"
            | "initialize" | "table_name_prefix" | "table_name_suffix" | "to_s" | "inspect" | "==" | "hash" | "eql?" | "<=>"
            | "to_param" | "to_partial_path" | "model_name"
    )
}

/// The sources' word counts, for the many "does anything call this method"
/// questions: [`Mentions::count`] is [`mentions_scan`] answered from one
/// pass over the text, built on the first ask. Each question scanned every
/// source, once per method of every class (`uncalled_bodies`).
pub(crate) struct Mentions<'a> {
    sources: &'a [SourceFile],
    words: std::cell::OnceCell<HashMap<&'a str, usize>>,
    /// The names the index does not answer (operators), scanned once each.
    scans: std::cell::RefCell<HashMap<String, usize>>,
}

impl<'a> Mentions<'a> {
    pub(crate) fn new(sources: &'a [SourceFile]) -> Self {
        Self { sources, words: std::cell::OnceCell::new(), scans: Default::default() }
    }

    fn mentioned(&self, name: &str) -> bool {
        self.count(name) > 0
    }

    /// How many times the sources name `name` as a word, outside its `def`.
    fn count(&self, name: &str) -> usize {
        if !is_word_name(name) {
            if let Some(&known) = self.scans.borrow().get(name) {
                return known;
            }
            let count = mentions_scan(name, self.sources);
            self.scans.borrow_mut().insert(name.to_string(), count);
            return count;
        }
        let words = self.words.get_or_init(|| {
            // Each chunk of sources counts on its own thread, and the totals are
            // sums merged in chunk order, so the schedule cannot change them.
            // `count_words` reads only its text: no thread-local compiler state,
            // which a worker would not have. Only this build runs in parallel;
            // `Mentions` itself (the `scans` memo) stays on the calling thread.
            // When `par::map` would not spawn workers (wasm / one core / one
            // chunk), stay on one accumulator — buffering every chunk map is
            // pure peak-memory cost with no schedule gain.
            let chunk = self.sources.len().div_ceil(64).max(1);
            let n_parts = self.sources.len().div_ceil(chunk);
            if crate::par::workers_for(n_parts) <= 1 {
                let mut words: HashMap<&'a str, usize> = HashMap::default();
                for source in self.sources {
                    count_words(&source.text, &mut words);
                }
                return words;
            }
            let parts: Vec<&'a [SourceFile]> = self.sources.chunks(chunk).collect();
            let mut words: HashMap<&'a str, usize> = HashMap::default();
            for part in crate::par::map(&parts, |part| {
                let mut words = HashMap::default();
                for source in *part {
                    count_words(&source.text, &mut words);
                }
                words
            }) {
                for (word, n) in part {
                    *words.entry(word).or_default() += n;
                }
            }
            words
        });
        words.get(name).copied().unwrap_or(0)
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A name `Mentions` answers from the index: word characters, then at most
/// one of `?`, `!`, `=`. Any other (an operator, `[]`, ``) is scanned.
fn is_word_name(name: &str) -> bool {
    let word = name.strip_suffix(['?', '!', '=']).unwrap_or(name);
    !word.is_empty() && word.chars().all(is_word_char)
}

/// Add to `words` what [`mentions_scan`] counts in `text` for each word
/// name: a maximal run of word characters not written after `def ` or
/// `def self.`, counted under the run itself when no `?`, `!` or `=`
/// follows it, and under the run plus that one character when what follows
/// that is not a word character or one of the three.
fn count_words<'a>(text: &'a str, words: &mut HashMap<&'a str, usize>) {
    let bytes = text.as_bytes();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !is_word_char(c) {
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(at, next)) = chars.peek() {
            if !is_word_char(next) {
                break;
            }
            end = at + next.len_utf8();
            chars.next();
        }
        let before = &text[..start];
        if before.ends_with("def ") || before.ends_with("def self.") {
            continue;
        }
        let suffix = bytes.get(end).copied().filter(|b| matches!(b, b'?' | b'!' | b'='));
        if suffix.is_none() {
            *words.entry(&text[start..end]).or_default() += 1;
            continue;
        }
        let after = &text[end + 1..];
        if !after.starts_with(|c: char| is_word_char(c) || matches!(c, '?' | '!' | '=')) {
            *words.entry(&text[start..end + 1]).or_default() += 1;
        }
    }
}

/// How many times the sources name `name` as a word, outside its `def`.
fn mentions_scan(name: &str, sources: &[SourceFile]) -> usize {
    sources
        .iter()
        .map(|source| {
            source
                .text
                .match_indices(name)
                .filter(|(at, _)| {
                    let before = &source.text[..*at];
                    let after = &source.text[at + name.len()..];
                    let word = !before.ends_with(|c: char| c.is_alphanumeric() || c == '_')
                        && !after.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '?' || c == '!' || c == '=');
                    word && !before.ends_with("def ") && !before.ends_with("def self.")
                })
                .count()
        })
        .sum()
}

/// Whether `text` calls one of `setters` (`Owner.name = v`, `send(:name=, v)`)
/// or sets the variable reflectively. Over-matching only keeps a value unfolded.
fn writes_through(text: &str, setters: &[&str], cvar: &str, dead: &dyn Fn(usize) -> bool) -> bool {
    for setter in setters {
        for (at, _) in text.match_indices(setter) {
            if dead(at) {
                continue;
            }
            let before = &text[..at];
            let after = &text[at + setter.len()..];
            let assigns = after.trim_start().starts_with('=') && !after.trim_start().starts_with("==");
            // `send(:name=, v)` and `send("name=", v)` name the setter as a Symbol or a String
            let named = before.ends_with(':') || before.ends_with('"') || before.ends_with('\'');
            if (assigns && before.ends_with('.') && !before.ends_with("def self.")) || (named && after.starts_with('=')) {
                return true;
            }
        }
    }
    text.contains("class_variable_set") && text.contains(&cvar[2..])
}

/// A class-body declaration the macro ran, as the model's own: written
/// back as Ruby and read the way the model's body is.
fn replay(statement: &Expr, owner: &ClassId, defaults: &mut indexmap::IndexMap<Symbol, Expr>) -> Vec<ModelBodyItem> {
    let unknown = || vec![ModelBodyItem::Unknown { expr: statement.clone(), leading_comments: Vec::new(), leading_blank_line: false }];
    let source = crate::emit::ruby::expr::emit_expr(statement);
    replay_text(&source, owner, defaults).unwrap_or_else(unknown)
}

fn replay_source(code: &str, owner: &ClassId, defaults: &mut indexmap::IndexMap<Symbol, Expr>) -> Vec<ModelBodyItem> {
    replay_text(code, owner, defaults).expect("read when the run evaluated it")
}

fn replay_text(source: &str, owner: &ClassId, defaults: &mut indexmap::IndexMap<Symbol, Expr>) -> Option<Vec<ModelBodyItem>> {
    let unknown = || None;
    let parsed = super::prism::parse_silent(source.as_bytes());
    if parsed.errors().next().is_some() {
        return unknown();
    }
    let Some(program) = parsed.node().as_program_node() else { return unknown() };
    let mut items = Vec::new();
    for stmt in program.statements().body().iter() {
        match super::model::ingest_model_body_items(&stmt, owner, "<class macro>", Vec::new(), Some(&mut *defaults), &super::model::ClassConsts::default()) {
            Ok(mut ingested) => {
                for item in &mut ingested {
                    mark_constants(item);
                }
                items.extend(ingested);
            }
            Err(_) => return unknown(),
        }
    }
    Some(items)
}

// A replayed statement's span points into a fragment the constant resolver never indexed, so its constants are read as written
fn mark_constants(item: &mut ModelBodyItem) {
    fn mark(e: &mut Expr) {
        if matches!(&*e.node, ExprNode::Const { .. }) {
            e.decisions |= crate::expr::GENERATED_CONST_REF;
        }
        e.node.for_each_child_mut(&mut mark);
    }
    if let ModelBodyItem::Unknown { expr, .. } = item {
        mark(expr);
    }
}

/// The instance methods the model lowering synthesizes for this model
/// (associations, declarations), which a generated `def` of the same
/// name would collide with.
fn synthesized_names(app: &App, model_index: usize) -> HashSet<Symbol> {
    let model = &app.models[model_index];
    let mut specs = crate::lower::controller_to_library::params::collect_specs(&app.controllers);
    specs.mark_file_fields(&app.models);
    let synthesized = crate::lower::model_to_library::build_methods(model, &app.models, &app.schema, &specs);
    let mut out: HashSet<Symbol> = crate::lower::model_to_library::build_class_info(model, &synthesized, app.schema.tables.get(&model.table.0))
        .instance_methods
        .into_keys()
        .collect();
    out.extend(synthesized.iter().filter(|m| m.receiver == MethodReceiver::Instance).map(|m| m.name.clone()));
    // Not a column's accessor: a model's own one wins over it (`push_user_methods`), as a generated one does in Ruby.
    if let Some(table) = app.schema.tables.get(&model.table.0) {
        for c in &table.columns {
            out.remove(&c.name);
            out.remove(&Symbol::from(format!("{}=", c.name.as_str())));
        }
    }
    out
}

/// Where each class name sits: the first model of the name, and every
/// library class of it, in order. Names are not written while the macros
/// expand, so the positions stay good (the methods they hold are read fresh,
/// as the expansion rewrites models as it goes).
struct Named {
    model: HashMap<ClassId, usize>,
    library: HashMap<ClassId, Vec<usize>>,
}

impl Named {
    fn of(app: &App) -> Self {
        let mut model: HashMap<ClassId, usize> = HashMap::default();
        for (i, m) in app.models.iter().enumerate() {
            model.entry(m.name.clone()).or_insert(i);
        }
        let mut library: HashMap<ClassId, Vec<usize>> = HashMap::default();
        for (i, lc) in app.library_classes.iter().enumerate() {
            library.entry(lc.name.clone()).or_default().push(i);
        }
        Named { model, library }
    }
}

fn ancestors(named: &Named, parents: &HashMap<ClassId, ClassId>, model: &ClassId) -> Vec<ClassId> {
    let mut out = Vec::new();
    let mut cur = parents.get(model);
    while let Some(p) = cur {
        if out.contains(p) || !(named.model.contains_key(p) || named.library.contains_key(p)) {
            break;
        }
        out.push(p.clone());
        cur = parents.get(p);
    }
    out
}

/// The class methods the ancestors define, the nearest one's winning.
fn class_method_table(app: &App, named: &Named, ancestors: &[ClassId]) -> HashMap<Symbol, MethodDef> {
    let mut table = HashMap::default();
    for id in ancestors.iter().rev() {
        let methods: Vec<&MethodDef> = match named.model.get(id) {
            Some(&i) => app.models[i].methods().collect(),
            None => named.library.get(id).into_iter().flatten().flat_map(|&i| app.library_classes[i].methods.iter()).collect(),
        };
        for m in methods.into_iter().filter(|m| m.receiver == MethodReceiver::Class) {
            table.insert(m.name.clone(), m.clone());
        }
    }
    table
}

fn reaches_definition(name: &Symbol, table: &HashMap<Symbol, MethodDef>, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let Some(def) = table.get(name) else { return false };
    let mut found = false;
    visit(&def.body, &mut |e| {
        if let ExprNode::Send { recv, method, .. } = &*e.node
            && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
        {
            found |= matches!(method.as_str(), "define_method" | "define_singleton_method")
                || (method != name && table.contains_key(method) && reaches_definition(method, table, depth + 1));
        }
    });
    found
}

fn visit(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| visit(c, f));
}

/// Constants the ancestors assign a list of Strings or Symbols
/// (`PROTECTED_KEYS = %w[var value].freeze`), for a guard that reads one.
fn literal_constants(app: &App, ancestors: &[ClassId]) -> HashMap<Symbol, Expr> {
    let mut out = HashMap::default();
    for id in ancestors.iter().rev() {
        let assigned: Vec<(Symbol, Expr)> = match app.models.iter().find(|m| &m.name == id) {
            Some(model) => model
                .body
                .iter()
                .filter_map(|item| match item {
                    ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
                        ExprNode::Assign { target: LValue::Const { path }, value } if path.len() == 1 => Some((path[0].clone(), value.clone())),
                        _ => None,
                    },
                    _ => None,
                })
                .collect(),
            None => app.library_classes.iter().filter(|lc| &lc.name == id).flat_map(|lc| lc.constants.iter().cloned()).collect(),
        };
        for (name, value) in &assigned {
            let value = match &*value.node {
                ExprNode::Send { recv: Some(r), method, args, .. } if method.as_str() == "freeze" && args.is_empty() => r,
                _ => value,
            };
            if let ExprNode::Array { elements, .. } = &*value.node
                && elements.iter().all(|e| matches!(&*e.node, ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }))
            {
                out.insert(name.clone(), value.clone());
            }
        }
    }
    out
}

fn slot_prefix(class: &str) -> String {
    let base: String = class.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    format!("ROUNDHOUSE_MACRO_{base}")
}

/// A value the macro computes from its arguments: a literal, or a
/// constant the expansion assigned (`truthy` when its shape says so).
#[derive(Clone, Debug)]
enum Val {
    Lit(Expr),
    Slot { name: Symbol, truthy: Option<bool> },
}

struct Run<'a> {
    table: &'a HashMap<Symbol, MethodDef>,
    constants: &'a HashMap<Symbol, Expr>,
    frozen: &'a HashMap<(String, Symbol), Expr>,
    cvar_readers: &'a HashMap<(String, String), Symbol>,
    /// The modules the class has included so far, by full name.
    included: Vec<String>,
    module_methods: &'a HashMap<String, Vec<MethodDef>>,
    /// What the class's own `extend` in this run added to its class side.
    extended: HashMap<Symbol, MethodDef>,
    /// Ruby source the run `class_eval`ed on the class, in order.
    evaluated: Vec<String>,
    /// Every class and module this program declares, by full name.
    known: &'a HashSet<String>,
    /// The module the running macro is defined in.
    scope: Option<Symbol>,
    prefix: &'a str,
    counter: usize,
    /// Class-body statements, in the order the macro runs them.
    out: Vec<Expr>,
    /// Indexes into `out` of the declarations to read as the model's
    /// body (`validate`, `cattr_accessor`, a mixin) rather than keep as
    /// a statement.
    replayed: HashSet<usize>,
    /// What `self.x = …` set, for a later read of `x`.
    class_attrs: HashMap<Symbol, Val>,
    /// A `cattr_*`/`mattr_*` declared in this run and not yet written,
    /// by name: the index of its declaration in `out`.
    unset_cattrs: HashMap<Symbol, usize>,
    methods: Vec<MethodDef>,
    aliases: Vec<(Symbol, Symbol, Span)>,
    /// One constant per captured String, so every method shares it.
    strings: HashMap<(String, usize), Symbol>,
    owner: Symbol,
    sources: &'a [SourceFile],
    /// The instance methods the class has without this run, by name, for
    /// `method_defined?`; not known for a model, whose attribute readers
    /// Rails defines later.
    instance_methods: Option<&'a HashSet<Symbol>>,
}

type Env = HashMap<Symbol, Val>;

impl Run<'_> {
    fn call(&mut self, name: &Symbol, args: &[Expr], block: Option<&Expr>, env: &Env, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("its calls nest too deep".into());
        }
        let def = self.method_def(name).cloned().ok_or_else(|| format!("`{}` is not one of the class's methods", name.as_str()))?;
        if !super::model_macros::supported_source_signature(&def, self.sources, true, true, true) {
            return Err(format!("`{}`'s parameters are not ones it binds", name.as_str()));
        }
        let mut bound = self.bind(&def, args, block, env)?;
        let statements: Vec<Expr> = match &*def.body.node {
            ExprNode::Seq { exprs } => exprs.clone(),
            _ => vec![def.body.clone()],
        };
        let inner = def.enclosing_class.clone().or_else(|| self.scope.clone());
        let outer = std::mem::replace(&mut self.scope, inner);
        let ran = statements.iter().try_for_each(|st| match self.exec(st, &mut bound, depth) {
            Ok(true) => Err(None),
            Ok(false) => Ok(()),
            Err(why) => Err(Some(why)),
        });
        self.scope = outer;
        match ran {
            Err(Some(why)) => Err(why),
            _ => Ok(()),
        }
    }

    fn method_def(&self, name: &Symbol) -> Option<&MethodDef> {
        self.table.get(name).or_else(|| self.extended.get(name))
    }

    fn bind(&mut self, def: &MethodDef, args: &[Expr], block: Option<&Expr>, env: &Env) -> Result<Env, String> {
        if def.params.iter().any(|p| p.from_kwrest) {
            return Err("its parameters include a keyword rest".into());
        }
        let mut bound = Env::default();
        // The block is captured as written: the defined bodies run it where they `instance_eval` or pass it.
        match (&def.block_param, block) {
            (Some(name), Some(block)) => {
                bound.insert(name.name.clone(), Val::Lit(block.clone()));
            }
            (Some(name), None) => {
                bound.insert(name.name.clone(), Val::Lit(lit(def.name_span, Literal::Nil)));
            }
            (None, Some(_)) => return Err("it is passed a block it does not name".into()),
            (None, None) => {}
        }
        let takes_keywords = def.params.iter().any(|p| p.keyword || p.from_keyword);
        // Not split off without keyword parameters: Ruby hands the trailing Hash to the last positional (`opts = {}`) or a `*rest`.
        let as_hash: Vec<Expr>;
        let args: &[Expr] = match args.last().map(|a| &*a.node) {
            Some(ExprNode::Hash { entries, kwargs: true }) if !takes_keywords => {
                let mut owned = args.to_vec();
                let last = owned.last_mut().expect("matched");
                *last = Expr::new(last.span, ExprNode::Hash { entries: entries.clone(), kwargs: false });
                as_hash = owned;
                &as_hash
            }
            _ => args,
        };
        let (positional, keywords): (&[Expr], Vec<(Symbol, Expr)>) = match args.last().map(|a| &*a.node) {
            Some(ExprNode::Hash { entries, kwargs: true }) if takes_keywords => {
                let mut kw = Vec::new();
                for (k, v) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
                        return Err("a keyword is not a Symbol".into());
                    };
                    kw.push((value.clone(), v.clone()));
                }
                (&args[..args.len() - 1], kw)
            }
            _ => (args, Vec::new()),
        };
        let mut keywords: HashMap<Symbol, Expr> = keywords.into_iter().collect();
        let mut positional = positional.iter();
        let required_after_rest = |at: usize| def.params[at + 1..].iter().filter(|p| !p.keyword && !p.from_keyword && p.default.is_none() && !p.rest).count();
        for (at, p) in def.params.iter().enumerate() {
            if p.rest {
                let mut rest = Vec::new();
                while positional.len() > required_after_rest(at) {
                    let e = positional.next().expect("counted").clone();
                    match self.eval(&e, env)? {
                        Val::Lit(v) => rest.push(v),
                        v => rest.push(self.read(&v, e.span)),
                    }
                }
                let span = rest.first().map_or(def.name_span, |e: &Expr| e.span);
                bound.insert(p.name.clone(), Val::Lit(Expr::new(span, ExprNode::Array { elements: rest, style: Default::default() })));
                continue;
            }
            let given = if p.keyword || p.from_keyword { keywords.remove(&p.name) } else { positional.next().cloned() };
            let val = match (given, &p.default) {
                (Some(e), _) => self.eval(&e, env)?,
                (None, Some(default)) => self.eval(default, &bound)?,
                (None, None) => return Err(format!("`{}` is not given", p.name.as_str())),
            };
            bound.insert(p.name.clone(), val);
        }
        if positional.next().is_some() || !keywords.is_empty() {
            return Err("it is given an argument it does not take".into());
        }
        Ok(bound)
    }

    /// Runs one statement; `true` when it returned from the macro.
    fn exec(&mut self, st: &Expr, env: &mut Env, depth: usize) -> Result<bool, String> {
        match &*st.node {
            ExprNode::Seq { exprs } => {
                for e in exprs {
                    if self.exec(e, env, depth)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            // The macro's value is not read: a class-body call discards it.
            ExprNode::Return { .. } => Ok(true),
            ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
                let v = self.eval(value, env)?;
                env.insert(name.clone(), v);
                Ok(false)
            }
            ExprNode::OpAssign { target: LValue::Var { name, .. }, op: crate::expr::OpAssignOp::OrOr, value } => {
                let current = env.get(name).cloned().ok_or_else(|| format!("`{}` is not bound", name.as_str()))?;
                match self.truthy(&current) {
                    Some(true) => {}
                    Some(false) => {
                        let v = self.eval(value, env)?;
                        env.insert(name.clone(), v);
                    }
                    None => return Err("a `||=` depends on a value known only when it runs".into()),
                }
                Ok(false)
            }
            ExprNode::If { cond, then_branch, else_branch } => match {
                let v = self.eval(cond, env)?;
                self.truthy(&v)
            } {
                Some(true) => self.exec(then_branch, env, depth),
                Some(false) => self.exec(else_branch, env, depth),
                None => Err("a condition depends on a value known only when it runs".into()),
            },
            ExprNode::Lit { value: Literal::Nil } => Ok(false),
            ExprNode::Send { recv, method, args, block, .. } if recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) => {
                match method.as_str() {
                    "define_method" | "define_singleton_method" => {
                        let receiver = if method.as_str() == "define_method" { MethodReceiver::Instance } else { MethodReceiver::Class };
                        // `define_method(:label, Mod.instance_method(:label))`: a copy of the app module's method.
                        if let [name, value] = args.as_slice()
                            && block.is_none()
                            && receiver == MethodReceiver::Instance
                        {
                            return self.define_from_instance_method(name, value, env).map(|()| false);
                        }
                        let [name] = args.as_slice() else { return Err("a definition is given other than one name".into()) };
                        let Some(block) = block else { return Err("a definition has no block".into()) };
                        if !definition_syntax_kept(st, self.sources) {
                            return Err("a definition uses syntax the expansion would erase".into());
                        }
                        self.define(receiver, name, block, env).map(|()| false)
                    }
                    "alias_method" => {
                        let [new_name, old_name] = args.as_slice() else { return Err("an alias is not given two names".into()) };
                        let new_name = self.literal_name(new_name, env)?;
                        let old_name = self.literal_name(old_name, env)?;
                        self.aliases.push((new_name, old_name, st.span));
                        Ok(false)
                    }
                    "raise" | "fail" => Err("it raises".into()),
                    "class_eval" | "class_exec" | "module_eval" if args.is_empty() => {
                        let Some(b) = block else { return Err("a `class_eval` has neither a block nor a String".into()) };
                        let ExprNode::Lambda { params, rest_param: None, body, .. } = &*b.node else {
                            return Err("a `class_eval` block takes parameters".into());
                        };
                        if !params.is_empty() {
                            return Err("a `class_eval` block takes parameters".into());
                        }
                        let statements: Vec<Expr> = match &*body.node {
                            ExprNode::Seq { exprs } => exprs.clone(),
                            _ => vec![body.clone()],
                        };
                        for statement in &statements {
                            // Not refused as a nested `def`: inside the class's own `class_eval` it is the class's method, as written.
                            if let Some(def) = statement.nested_def() {
                                let mut def = def.clone();
                                def.enclosing_class = Some(self.owner.clone());
                                self.methods.push(def);
                                continue;
                            }
                            if self.exec(statement, env, depth)? {
                                return Ok(true);
                            }
                        }
                        Ok(false)
                    }
                    "class_eval" | "module_eval" if block.is_none() && !args.is_empty() => {
                        let Some(Val::Lit(code)) = self.fold(&args[0], env) else {
                            return Err("it `class_eval`s a String known only when it runs".into());
                        };
                        let ExprNode::Lit { value: Literal::Str { value: code } } = &*code.node else {
                            return Err("it `class_eval`s other than a String".into());
                        };
                        if replay_text(code, &ClassId(self.owner.clone()), &mut indexmap::IndexMap::new()).is_none() {
                            return Err("the String it `class_eval`s is not a class body this can read".into());
                        }
                        self.evaluated.push(code.clone());
                        Ok(false)
                    }
                    "include" | "extend"
                        if block.is_none() && !args.is_empty() && args.iter().all(|a| matches!(&*a.node, ExprNode::Const { .. })) =>
                    {
                        let mut statement = st.clone();
                        if let ExprNode::Send { args, .. } = &mut *statement.node {
                            for arg in args.iter_mut() {
                                let ExprNode::Const { path } = &mut *arg.node else { continue };
                                let Some(resolved) = self.resolve_module(path) else {
                                    return Err(format!(
                                        "its `{}` is not a module this program defines",
                                        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")
                                    ));
                                };
                                let full = resolved.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                                if method.as_str() == "include" {
                                    self.included.push(full);
                                } else if let Some(methods) = self.module_methods.get(&full) {
                                    for m in methods {
                                        let mut def = m.clone();
                                        def.receiver = MethodReceiver::Class;
                                        self.extended.entry(def.name.clone()).or_insert(def);
                                    }
                                }
                                *path = resolved;
                            }
                        }
                        self.declare(statement);
                        Ok(false)
                    }
                    name if name.ends_with('=')
                        && !matches!(name, "==" | "!=" | "<=" | ">=" | "===")
                        && recv.is_some()
                        && block.is_none()
                        && args.len() == 1
                        // Not run when the class's own `extend` brought the writer: the model replays the write, which calls it there (ancestry's `orphan_strategy=`).
                        && self.table.get(method).is_none_or(attr_writer) =>
                    {
                        let v = self.eval(&args[0], env)?;
                        let read = self.read(&v, args[0].span);
                        let attr = Symbol::from(name.trim_end_matches('='));
                        self.class_attrs.insert(attr.clone(), v);
                        // Not replayed as the call: the writer is the macro module's, which sets the class's own `@x`.
                        if let Some(def) = self.method_def(method)
                            && let Some(ivar) = writer_ivar(def)
                        {
                            let value = match self.class_attrs.get(&attr) {
                                Some(Val::Lit(e)) => e.clone(),
                                _ => read,
                            };
                            self.out.push(Expr::new(st.span, ExprNode::Assign { target: LValue::Ivar { name: ivar }, value }));
                            return Ok(false);
                        }
                        // The first write after the declaration is its default, which the model's cattr reads as one
                        if let Some(index) = self.unset_cattrs.remove(&attr)
                            && let ExprNode::Send { args, .. } = &mut *self.out[index].node
                        {
                            let key = lit(read.span, Literal::Sym { value: Symbol::from("default") });
                            args.push(Expr::new(read.span, ExprNode::Hash { entries: vec![(key, read)], kwargs: true }));
                            return Ok(false);
                        }
                        let mut assigned = st.clone();
                        if let ExprNode::Send { args, .. } = &mut *assigned.node {
                            args[0] = read;
                        }
                        self.declare(assigned);
                        Ok(false)
                    }
                    // Not expanded: a dedicated lowering claims such an installer (`has_markdown`), and a name it cannot read fails closed.
                    "has_one" | "has_many" | "belongs_to" | "has_and_belongs_to_many" | "scope"
                        if args.first().is_some_and(|a| matches!(&*a.node, ExprNode::Send { recv: Some(r), method, .. }
                            if method.as_str() == "to_sym" && matches!(&*r.node, ExprNode::StringInterp { .. }))) =>
                    {
                        Err(format!("it declares a `{}` whose name is built from its arguments", method.as_str()))
                    }
                    name if CLASS_BODY_DECLARATIONS.contains(&name) && block.is_none() && self.method_def(method).is_none() => {
                        let declared = self.substitute(st, env, &HashSet::default())?;
                        if matches!(name, "cattr_accessor" | "cattr_reader" | "mattr_accessor" | "mattr_reader")
                            && let ExprNode::Send { args, .. } = &*declared.node
                            && let [arg] = args.as_slice()
                            && let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node
                        {
                            self.unset_cattrs.insert(value.clone(), self.out.len());
                        }
                        self.declare(declared);
                        Ok(false)
                    }
                    _ if self.method_def(method).is_some() => {
                        let args: Vec<Expr> = args.clone();
                        // A block forwarded on (`&definition`) is the one this run holds; a literal one is passed as written.
                        let block = match block.as_ref().map(|b| &*b.node) {
                            None => None,
                            Some(ExprNode::Var { name, .. }) => match env.get(name) {
                                Some(Val::Lit(held)) if matches!(&*held.node, ExprNode::Lambda { .. }) => Some(held.clone()),
                                Some(Val::Lit(held)) if matches!(&*held.node, ExprNode::Lit { value: Literal::Nil }) => None,
                                _ => return Err(format!("it passes `{}` a block known only when it runs", method.as_str())),
                            },
                            Some(ExprNode::Lambda { .. }) => Some(self.substitute(block.as_ref().unwrap(), env, &HashSet::default())?),
                            Some(_) => return Err(format!("it passes `{}` a block known only when it runs", method.as_str())),
                        };
                        self.call(method, &args, block.as_ref(), env, depth + 1).map(|()| false)
                    }
                    _ => Err(format!("it calls `{}`", method.as_str())),
                }
            }
            ExprNode::Assign { target: LValue::Attr { recv, name }, value } if matches!(&*recv.node, ExprNode::SelfRef) => {
                let v = self.eval(value, env)?;
                let read = self.read(&v, value.span);
                self.class_attrs.insert(name.clone(), v);
                let mut assigned = st.clone();
                if let ExprNode::Assign { value, .. } = &mut *assigned.node {
                    *value = read;
                }
                self.declare(assigned);
                Ok(false)
            }
            ExprNode::Send { recv: Some(r), method, args, block: None, .. }
                if matches!(method.as_str(), "reverse_merge!" | "merge!" | "update")
                    && let ExprNode::Var { name, .. } = &*r.node
                    && let [other] = args.as_slice() =>
            {
                let (Some(Val::Lit(mine)), Some(Val::Lit(theirs))) = (env.get(name).cloned(), self.fold(other, env)) else {
                    return Err(format!("a `{}` merges a value known only when it runs", method.as_str()));
                };
                let (ExprNode::Hash { entries: mine_entries, kwargs }, ExprNode::Hash { entries: their_entries, .. }) = (&*mine.node, &*theirs.node) else {
                    return Err(format!("a `{}` merges other than two Hashes", method.as_str()));
                };
                let mut merged = mine_entries.clone();
                for (k, v) in their_entries {
                    let ExprNode::Lit { value: key } = &*k.node else { return Err("a merged key is not a literal".into()) };
                    let mut at = None;
                    for (i, (mk, _)) in merged.iter().enumerate() {
                        let ExprNode::Lit { value: mkey } = &*mk.node else { return Err("a merged key is not a literal".into()) };
                        if literal_eq(mkey, key).ok_or_else(|| "a merged key is not comparable".to_string())? {
                            at = Some(i);
                        }
                    }
                    match (at, method.as_str()) {
                        (Some(_), "reverse_merge!") => {}
                        (Some(i), _) => merged[i].1 = v.clone(),
                        (None, _) => merged.push((k.clone(), v.clone())),
                    }
                }
                env.insert(name.clone(), Val::Lit(Expr::new(mine.span, ExprNode::Hash { entries: merged, kwargs: *kwargs })));
                Ok(false)
            }
            // Not replayed as written: in the model's body `@@x` would name the model's class variable, not the macro module's.
            ExprNode::Send { recv: Some(r), method, args, block: None, parenthesized }
                if let ExprNode::Var { name, .. } = &*r.node
                    && name.as_str().starts_with("@@") =>
            {
                let module = self.scope.clone().ok_or_else(|| "it writes a class variable outside a module".to_string())?;
                let reader = self
                    .cvar_readers
                    .get(&(module.as_str().to_string(), name.as_str().to_string()))
                    .ok_or_else(|| format!("its module has no reader for `{}`", name.as_str()))?;
                let mut folded = Vec::new();
                for a in args {
                    match self.eval(a, env)? {
                        Val::Lit(e) => folded.push(e),
                        v => folded.push(self.read(&v, a.span)),
                    }
                }
                let path = std::iter::once(Symbol::from("")).chain(module.as_str().split("::").map(Symbol::from)).collect();
                let mut owner = Expr::new(r.span, ExprNode::Const { path });
                owner.decisions |= crate::expr::GENERATED_CONST_REF;
                let recv = Expr::new(r.span, ExprNode::Send { recv: Some(owner), method: reader.clone(), args: Vec::new(), block: None, parenthesized: false });
                let mut statement = Expr::new(st.span, ExprNode::Send { recv: Some(recv), method: method.clone(), args: folded, block: None, parenthesized: *parenthesized });
                statement.decisions |= crate::expr::REPLAYED_CLASS_BODY;
                self.out.push(statement);
                Ok(false)
            }
            // A loop over what the macro was given: each element runs the block, as Ruby does.
            ExprNode::Send { recv: Some(r), method, args, block: Some(b), .. }
                if matches!(method.as_str(), "each" | "each_pair") && args.is_empty() =>
            {
                let Some(Val::Lit(over)) = self.fold(r, env) else { return Err("it loops over a value known only when it runs".into()) };
                let ExprNode::Lambda { params, rest_param: None, body, .. } = &*b.node else { return Err("a loop's block takes other than plain parameters".into()) };
                let rows: Vec<Vec<Expr>> = match &*over.node {
                    ExprNode::Hash { entries, .. } => entries.iter().map(|(k, v)| vec![k.clone(), v.clone()]).collect(),
                    ExprNode::Array { elements, .. } => elements.iter().map(|e| vec![e.clone()]).collect(),
                    _ => return Err("it loops over a value that is not a literal Hash or Array".into()),
                };
                // Not a key: Ruby hands a one-parameter block over a Hash the `[key, value]` pair
                if matches!(&*over.node, ExprNode::Hash { .. }) && params.len() != 2 {
                    return Err("a loop over a Hash binds other than its key and value".into());
                }
                for row in rows {
                    if params.len() > row.len() {
                        return Err("a loop's block takes more parameters than each element has".into());
                    }
                    let mut inner = env.clone();
                    for (param, value) in params.iter().zip(row.iter()) {
                        inner.insert(param.clone(), Val::Lit(value.clone()));
                    }
                    if self.exec(body, &mut inner, depth)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            // The class's own state: run where the macro runs, in the calling class's body.
            _ if touches_class_state(st) => {
                let substituted = self.substitute(st, env, &HashSet::default())?;
                self.out.push(substituted);
                Ok(false)
            }
            _ => Err(format!("a statement is not one it can run: {}", describe(st))),
        }
    }

    // Rooted, not as written: the model's body would read `Role` as its own app's constant, not the macro module's.
    fn resolve_module(&self, path: &[Symbol]) -> Option<Vec<Symbol>> {
        let rooted = path.first().is_some_and(|s| s.as_str().is_empty());
        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
        let full = |name: &str| std::iter::once(Symbol::from("")).chain(name.split("::").map(Symbol::from)).collect();
        if !rooted {
            let mut scope = self.scope.as_ref().map(|s| s.as_str().to_string());
            while let Some(outer) = scope {
                let candidate = format!("{outer}::{written}");
                if self.known.contains(&candidate) {
                    return Some(full(&candidate));
                }
                scope = outer.rsplit_once("::").map(|(o, _)| o.to_string());
            }
        }
        self.known.contains(&written).then(|| full(&written))
    }

    fn declare(&mut self, statement: Expr) {
        self.replayed.insert(self.out.len());
        self.out.push(statement);
    }

    fn literal_name(&mut self, name: &Expr, env: &Env) -> Result<Symbol, String> {
        match self.fold(name, env) {
            Some(Val::Lit(e)) => match &*e.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Ok(value.clone()),
                ExprNode::Lit { value: Literal::Str { value } } => Ok(Symbol::from(value.as_str())),
                _ => Err("a defined name is not a name".into()),
            },
            _ => Err("a defined name is known only when it runs".into()),
        }
    }

    /// `define_method(name, Mod.instance_method(name2))` where `Mod` is an app
    /// module that defines `name2`: that method's `def`, copied in as `name`.
    fn define_from_instance_method(&mut self, name: &Expr, value: &Expr, env: &Env) -> Result<(), String> {
        let ExprNode::Send { recv: Some(module), method, args, block: None, .. } = &*value.node else {
            return Err("a definition is given other than one name".into());
        };
        let [source] = args.as_slice() else { return Err("a definition is given other than one name".into()) };
        if method.as_str() != "instance_method" {
            return Err("a definition is given other than one name".into());
        }
        let path = match self.fold(module, env) {
            Some(Val::Lit(e)) => match &*e.node {
                ExprNode::Const { path } => path.clone(),
                _ => return Err("an `instance_method` receiver is not a module".into()),
            },
            _ => return Err("an `instance_method` receiver is known only when it runs".into()),
        };
        let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
        let Some(resolved) = self.resolve_module(&path) else {
            return Err(format!("its `{written}` is not a module this program defines"));
        };
        let full = resolved.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
        let source = self.literal_name(source, env)?;
        let name = self.literal_name(name, env)?;
        if !valid_def_name(&name) {
            return Err(format!("`{}` is not a method name", name.as_str()));
        }
        let Some(found) = self.module_methods.get(&full).and_then(|methods| methods.iter().find(|m| m.name == source)) else {
            return Err(format!("`{full}` defines no `{}`", source.as_str()));
        };
        let mut def = found.clone();
        def.name = name;
        def.receiver = MethodReceiver::Instance;
        def.enclosing_class = Some(self.owner.clone());
        self.methods.push(def);
        Ok(())
    }

    fn define(&mut self, receiver: MethodReceiver, name: &Expr, block: &Expr, env: &Env) -> Result<(), String> {
        let name = self.literal_name(name, env)?;
        if !valid_def_name(&name) {
            return Err(format!("`{}` is not a method name", name.as_str()));
        }
        let ExprNode::Lambda { params, rest_param, extra_params, block_param: None, body, .. } = &*block.node else {
            return Err("a definition's block takes a block".into());
        };
        let mut shadow: HashSet<Symbol> = params.iter().cloned().chain(rest_param.iter().cloned()).collect();
        shadow.extend(extra_params.iter().map(|p| p.name.clone()));
        collect_locals(body, &mut shadow);
        if let Some(captured) = env.keys().find(|k| shadow.contains(*k) && assigns(body, k)) {
            return Err(format!("the defined `{}` reassigns its captured `{}`", name.as_str(), captured.as_str()));
        }
        let mut method_params: Vec<Param> = params.iter().map(|p| plain_param(p.clone())).collect();
        if let Some(rest) = rest_param {
            let mut p = plain_param(rest.clone());
            p.rest = true;
            method_params.push(p);
        }
        for p in extra_params {
            let mut p = p.clone();
            if let Some(d) = &p.default {
                p.default = Some(self.substitute(d, env, &HashSet::default())?);
            }
            method_params.push(p);
        }
        let body = self.substitute(body, env, &shadow)?;
        // Ruby makes these private whatever defines them.
        let visibility = if receiver == MethodReceiver::Instance && matches!(name.as_str(), "initialize" | "initialize_copy" | "initialize_dup" | "initialize_clone") {
            MethodVisibility::Private
        } else {
            MethodVisibility::Public
        };
        self.methods.push(MethodDef {
            name,
            receiver,
            visibility,
            params: method_params,
            unsupported_formals: None,
            has_anonymous_block: false,
            block_param: None,
            name_span: block.span,
            body,
            signature: None,
            effects: Default::default(),
            enclosing_class: Some(self.owner.clone()),
            // A `def`, whatever macro defined it: not the accessor kind of some other class method in the table.
            kind: crate::dialect::AccessorKind::Method,
            is_async: false,
            mutates_self: false,
        });
        Ok(())
    }

    /// What the macro computes from `e`, folded where its inputs are
    /// literal, otherwise assigned to a constant at this point.
    fn eval(&mut self, e: &Expr, env: &Env) -> Result<Val, String> {
        if let Some(v) = self.fold(e, env) {
            return Ok(v);
        }
        let value = self.substitute(e, env, &HashSet::default())?;
        let truthy = matches!(&*value.node, ExprNode::Hash { .. } | ExprNode::Array { .. } | ExprNode::StringInterp { .. }).then_some(true);
        Ok(Val::Slot { name: self.slot(value), truthy })
    }

    fn fold(&mut self, e: &Expr, env: &Env) -> Option<Val> {
        match &*e.node {
            ExprNode::Lit { .. } | ExprNode::SelfRef => Some(Val::Lit(e.clone())),
            // A module this program defines is itself (`borrow(Gadget, :label)`).
            ExprNode::Const { path } if self.resolve_module(path).is_some() => Some(Val::Lit(e.clone())),
            ExprNode::Var { name, .. } => env.get(name).cloned(),
            ExprNode::Send { recv: Some(r), method, args, block: None, .. }
                if args.is_empty()
                    && let ExprNode::Const { path } = &*r.node
                    && let Some(value) = self.frozen.get(&(
                        path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::"),
                        method.clone(),
                    )) =>
            {
                Some(Val::Lit(Expr { span: e.span, ..value.clone() }))
            }
            // Not answered for a name `Object` may hold: whether Ruby counts it turns on its visibility.
            ExprNode::Send { recv, method, args, block: None, .. }
                if method.as_str() == "method_defined?"
                    && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                    && self.method_def(method).is_none()
                    && let [arg] = args.as_slice()
                    && let Some(Val::Lit(name)) = self.fold(arg, env)
                    && let ExprNode::Lit { value: Literal::Sym { value: name } } = &*name.node =>
            {
                let defined = self.instance_methods?.contains(name)
                    || self.methods.iter().any(|m| m.receiver == MethodReceiver::Instance && m.visibility != MethodVisibility::Private && &m.name == name);
                if !defined && crate::emit::audit::OBJECT_METHODS.contains(&name.as_str()) {
                    return None;
                }
                Some(Val::Lit(lit(e.span, Literal::Bool { value: defined })))
            }
            ExprNode::Hash { entries, kwargs } => {
                let mut folded = Vec::new();
                for (k, v) in entries {
                    let (Val::Lit(k), Val::Lit(v)) = (self.fold(k, env)?, self.fold(v, env)?) else { return None };
                    folded.push((k, v));
                }
                Some(Val::Lit(Expr::new(e.span, ExprNode::Hash { entries: folded, kwargs: *kwargs })))
            }
            ExprNode::Array { elements, style } => {
                let mut folded = Vec::new();
                for el in elements {
                    let Val::Lit(el) = self.fold(el, env)? else { return None };
                    folded.push(el);
                }
                Some(Val::Lit(Expr::new(e.span, ExprNode::Array { elements: folded, style: style.clone() })))
            }
            ExprNode::Send { recv, method, args, block: None, .. }
                if args.is_empty() && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) && self.class_attrs.contains_key(method) =>
            {
                self.class_attrs.get(method).cloned()
            }
            ExprNode::Send { recv, method, args, block: None, .. }
                if args.is_empty()
                    && matches!(method.as_str(), "to_s" | "name")
                    && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                    && !self.class_attrs.contains_key(method)
                    && self.method_def(method).is_none() =>
            {
                Some(Val::Lit(lit(e.span, Literal::Str { value: self.owner.as_str().to_string() })))
            }
            ExprNode::Send { recv, method, args, block: None, .. }
                if args.is_empty()
                    && method.as_str() == "included_modules"
                    && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                    && self.method_def(method).is_none() =>
            {
                let elements = self
                    .included
                    .iter()
                    .map(|name| Expr::new(e.span, ExprNode::Const { path: std::iter::once(Symbol::from("")).chain(name.split("::").map(Symbol::from)).collect() }))
                    .collect();
                Some(Val::Lit(Expr::new(e.span, ExprNode::Array { elements, style: Default::default() })))
            }
            ExprNode::Send { recv, method, args, block: None, .. }
                if args.is_empty()
                    && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                    && !self.class_attrs.contains_key(method)
                    && !self.methods.iter().any(|m| &m.name == method)
                    && let Some(def) = self.method_def(method)
                    && def.params.is_empty()
                    && let ExprNode::Lit { .. } = &*match &*def.body.node {
                        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
                        _ => &def.body,
                    }
                    .node =>
            {
                let body = match &*def.body.node {
                    ExprNode::Seq { exprs } if exprs.len() == 1 => exprs[0].clone(),
                    _ => def.body.clone(),
                };
                Some(Val::Lit(Expr { span: e.span, ..body }))
            }
            ExprNode::Send { recv: Some(r), method, args, block: None, .. }
                if args.is_empty() && matches!(method.as_str(), "to_a" | "flatten" | "compact" | "uniq") =>
            {
                let Val::Lit(recv) = self.fold(r, env)? else { return None };
                let ExprNode::Array { elements, style } = &*recv.node else { return None };
                let mut out: Vec<Expr> = Vec::new();
                for el in elements {
                    match (method.as_str(), &*el.node) {
                        ("flatten", ExprNode::Array { elements: inner, .. }) => {
                            if inner.iter().any(|i| matches!(&*i.node, ExprNode::Array { .. })) {
                                return None;
                            }
                            out.extend(inner.iter().cloned());
                        }
                        ("compact", ExprNode::Lit { value: Literal::Nil }) => {}
                        ("uniq", ExprNode::Lit { value }) => {
                            let mut seen = false;
                            for o in &out {
                                let ExprNode::Lit { value: o } = &*o.node else { return None };
                                seen |= literal_eq(o, value)?;
                            }
                            if !seen {
                                out.push(el.clone());
                            }
                        }
                        ("uniq", _) => return None,
                        _ => out.push(el.clone()),
                    }
                }
                Some(Val::Lit(Expr::new(e.span, ExprNode::Array { elements: out, style: style.clone() })))
            }
            ExprNode::Send { recv: Some(r), method, args, block: Some(b), .. }
                if args.is_empty() && matches!(method.as_str(), "map" | "collect") =>
            {
                let Val::Lit(recv) = self.fold(r, env)? else { return None };
                let ExprNode::Array { elements, style } = &*recv.node else { return None };
                let ExprNode::Lambda { params, rest_param: None, body, .. } = &*b.node else { return None };
                let [param] = params.as_slice() else { return None };
                let mut out = Vec::new();
                for el in elements {
                    let mut inner = env.clone();
                    inner.insert(param.clone(), Val::Lit(el.clone()));
                    let Val::Lit(v) = self.fold(body, &inner)? else { return None };
                    out.push(v);
                }
                Some(Val::Lit(Expr::new(e.span, ExprNode::Array { elements: out, style: style.clone() })))
            }
            ExprNode::Send { recv: Some(r), method, args, block: Some(b), .. }
                if args.is_empty() && matches!(method.as_str(), "reject" | "select" | "filter") =>
            {
                let Val::Lit(recv) = self.fold(r, env)? else { return None };
                let ExprNode::Hash { entries, kwargs } = &*recv.node else { return None };
                let ExprNode::Lambda { params, rest_param: None, body, .. } = &*b.node else { return None };
                let [key_param, value_param] = params.as_slice() else { return None };
                let mut kept = Vec::new();
                for (k, v) in entries {
                    let mut inner = env.clone();
                    inner.insert(key_param.clone(), Val::Lit(k.clone()));
                    inner.insert(value_param.clone(), Val::Lit(v.clone()));
                    let answer = self.fold(body, &inner)?;
                    if self.truthy(&answer)? == (method.as_str() != "reject") {
                        kept.push((k.clone(), v.clone()));
                    }
                }
                Some(Val::Lit(Expr::new(e.span, ExprNode::Hash { entries: kept, kwargs: *kwargs })))
            }
            ExprNode::Send { recv: Some(r), method, args, block: None, .. } if self.fold_query(r, method, args, env).is_some() => {
                self.fold_query(r, method, args, env).map(|value| Val::Lit(lit(e.span, value)))
            }
            ExprNode::Send { recv: Some(r), method, args, block: None, .. } if method.as_str() == "[]" || method.as_str() == "presence" => {
                let Val::Lit(recv) = self.fold(r, env)? else { return None };
                match (&*recv.node, method.as_str(), args.as_slice()) {
                    (ExprNode::Hash { entries, .. }, "[]", [key]) => {
                        let Val::Lit(key) = self.fold(key, env)? else { return None };
                        let ExprNode::Lit { value: key } = &*key.node else { return None };
                        for (k, v) in entries {
                            let ExprNode::Lit { value: k } = &*k.node else { return None };
                            if literal_eq(k, key)? {
                                return Some(Val::Lit(v.clone()));
                            }
                        }
                        Some(Val::Lit(lit(e.span, Literal::Nil)))
                    }
                    (_, "presence", []) => Some(Val::Lit(if blank(&recv)? { lit(e.span, Literal::Nil) } else { recv.clone() })),
                    _ => None,
                }
            }
            ExprNode::StringInterp { .. } => {
                let mut text = String::new();
                let mut ok = true;
                let ExprNode::StringInterp { parts } = &*e.node else { unreachable!() };
                for part in parts {
                    match part {
                        crate::expr::InterpPart::Text { value } => text.push_str(value),
                        crate::expr::InterpPart::Expr { expr } => match self.fold(expr, env).and_then(|v| lit_text(&v)) {
                            Some(t) => text.push_str(&t),
                            None => ok = false,
                        },
                    }
                }
                ok.then(|| Val::Lit(lit(e.span, Literal::Str { value: text })))
            }
            ExprNode::BoolOp { op, left, right, .. } => {
                let l = self.fold(left, env)?;
                let lt = self.truthy(&l)?;
                let or = matches!(op, crate::expr::BoolOpKind::Or);
                if lt == or { Some(l) } else { self.fold(right, env) }
            }
            ExprNode::Send { recv: Some(r), method, args, block: None, .. } => {
                let recv = self.fold(r, env)?;
                let Val::Lit(recv) = recv else { return None };
                let ExprNode::Lit { value } = &*recv.node else { return None };
                if let Literal::Str { value: text } = value
                    && let Some(folded) = inflected(method.as_str(), text, args, |a| self.fold(a, env))
                {
                    return Some(Val::Lit(lit(e.span, Literal::Str { value: folded })));
                }
                match (method.as_str(), value, args.as_slice()) {
                    ("to_s", _, []) => lit_text(&Val::Lit(recv.clone())).map(|t| Val::Lit(lit(e.span, Literal::Str { value: t }))),
                    ("to_sym", _, []) => lit_text(&Val::Lit(recv.clone())).map(|t| Val::Lit(lit(e.span, Literal::Sym { value: Symbol::from(t.as_str()) }))),
                    ("nil?", v, []) => Some(Val::Lit(lit(e.span, Literal::Bool { value: matches!(v, Literal::Nil) }))),
                    ("==" | "!=", _, [other]) => {
                        let Val::Lit(other) = self.fold(other, env)? else { return None };
                        let ExprNode::Lit { value: o } = &*other.node else { return None };
                        let same = literal_eq(value, o)?;
                        Some(Val::Lit(lit(e.span, Literal::Bool { value: same == (method.as_str() == "==") })))
                    }
                    ("in?", _, [list]) => {
                        let ExprNode::Const { path } = &*list.node else { return None };
                        let [name] = path.as_slice() else { return None };
                        let ExprNode::Array { elements, .. } = &*self.constants.get(name)?.node else { return None };
                        let mut found = false;
                        for el in elements {
                            let ExprNode::Lit { value: o } = &*el.node else { return None };
                            found |= literal_eq(value, o)?;
                        }
                        Some(Val::Lit(lit(e.span, Literal::Bool { value: found })))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A question with a literal answer about a literal: `is_a?`,
    /// `present?`, `blank?`, `include?`, `!`.
    fn fold_query(&mut self, r: &Expr, method: &Symbol, args: &[Expr], env: &Env) -> Option<Literal> {
        if !matches!(method.as_str(), "is_a?" | "kind_of?" | "present?" | "blank?" | "empty?" | "include?" | "!") {
            return None;
        }
        let Val::Lit(recv) = self.fold(r, env)? else { return None };
        let answer = match (method.as_str(), args) {
            ("is_a?" | "kind_of?", [class]) => {
                let ExprNode::Const { path } = &*class.node else { return None };
                let written: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
                let kind = match &*recv.node {
                    ExprNode::Hash { .. } => "Hash",
                    ExprNode::Array { .. } => "Array",
                    ExprNode::Lit { value: Literal::Str { .. } } => "String",
                    ExprNode::Lit { value: Literal::Sym { .. } } => "Symbol",
                    ExprNode::Lit { value: Literal::Int { .. } } => "Integer",
                    ExprNode::Lit { value: Literal::Nil } => "NilClass",
                    _ => return None,
                };
                match written.as_slice() {
                    [name] if *name == kind => true,
                    [name] if matches!(*name, "Hash" | "Array" | "String" | "Symbol" | "Integer" | "NilClass") => false,
                    _ => return None,
                }
            }
            ("present?", []) => !blank(&recv)?,
            ("blank?", []) => blank(&recv)?,
            ("empty?", []) => match &*recv.node {
                ExprNode::Hash { entries, .. } => entries.is_empty(),
                ExprNode::Array { elements, .. } => elements.is_empty(),
                ExprNode::Lit { value: Literal::Str { value } } => value.is_empty(),
                _ => return None,
            },
            ("include?", [needle]) if let ExprNode::Const { path } = &*needle.node => {
                let ExprNode::Array { elements, .. } = &*recv.node else { return None };
                // Not a module this program defines: its presence among Rails' own ancestors is not known here.
                let wanted = self.resolve_module(path)?;
                let name = |p: &[Symbol]| p.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                elements.iter().any(|el| matches!(&*el.node, ExprNode::Const { path } if name(path) == name(&wanted)))
            }
            ("include?", [needle]) => {
                let ExprNode::Array { elements, .. } = &*recv.node else { return None };
                let Val::Lit(needle) = self.fold(needle, env)? else { return None };
                let ExprNode::Lit { value: needle } = &*needle.node else { return None };
                let mut found = false;
                for el in elements {
                    let ExprNode::Lit { value } = &*el.node else { return None };
                    found |= literal_eq(value, needle)?;
                }
                found
            }
            ("!", []) => !self.truthy(&Val::Lit(recv))?,
            _ => return None,
        };
        Some(Literal::Bool { value: answer })
    }

    fn truthy(&self, v: &Val) -> Option<bool> {
        match v {
            Val::Lit(e) => match &*e.node {
                ExprNode::Lit { value: Literal::Nil } => Some(false),
                ExprNode::Lit { value: Literal::Bool { value } } => Some(*value),
                _ => Some(true),
            },
            Val::Slot { truthy, .. } => *truthy,
        }
    }

    fn slot(&mut self, value: Expr) -> Symbol {
        self.counter += 1;
        let name = Symbol::from(format!("{}_{}", self.prefix, self.counter));
        let span = value.span;
        self.out.push(Expr::new(span, ExprNode::Assign { target: LValue::Const { path: vec![name.clone()] }, value }));
        name
    }

    /// `e` with each captured name replaced by what it holds, and a
    /// literal-named `send` / `__send__` on `self` called directly.
    fn substitute(&mut self, e: &Expr, env: &Env, shadow: &HashSet<Symbol>) -> Result<Expr, String> {
        let mut out = e.clone();
        let mut failed = None;
        self.substitute_in(&mut out, env, shadow, &mut failed);
        match failed {
            Some(why) => Err(why),
            None => Ok(out),
        }
    }

    fn substitute_in(&mut self, e: &mut Expr, env: &Env, shadow: &HashSet<Symbol>, failed: &mut Option<String>) {
        if let ExprNode::Var { name, .. } = &*e.node
            && !shadow.contains(name)
            && let Some(v) = env.get(name).cloned()
        {
            *e = self.read(&v, e.span);
            return;
        }
        if matches!(&*e.node, ExprNode::Lambda { .. }) {
            let mut inner = shadow.clone();
            if let ExprNode::Lambda { params, rest_param, extra_params, .. } = &*e.node {
                inner.extend(params.iter().cloned().chain(rest_param.iter().cloned()).chain(extra_params.iter().map(|p| p.name.clone())));
            }
            e.node.for_each_child_mut(&mut |c| self.substitute_in(c, env, &inner, failed));
            return;
        }
        e.node.for_each_child_mut(&mut |c| self.substitute_in(c, env, shadow, failed));
        // Only a name being spelled (`:"#{name}="`): other folds read the macro owner's constants, not this body's.
        if (matches!(&*e.node, ExprNode::StringInterp { .. }) || matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "to_sym"))
            && let Some(Val::Lit(folded)) = self.fold(e, &Env::default())
        {
            *e = folded;
        }
        collapse_literal_send(e);
        // `instance_eval(&definition)` on the defining object, once the block is the call's own: its body, run where it is.
        if let ExprNode::Send { recv, method, args, block: Some(block), .. } = &*e.node
            && matches!(method.as_str(), "instance_eval" | "instance_exec")
            && args.is_empty()
            && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
            && let ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. } = &*block.node
            && params.is_empty()
            && extra_params.is_empty()
        {
            *e = body.clone();
        }
    }

    fn read(&mut self, v: &Val, span: Span) -> Expr {
        match v {
            // Not inlined: a captured String is one object every method shares, where a literal allocates a fresh one per call.
            Val::Lit(e) if matches!(&*e.node, ExprNode::Lit { value: Literal::Str { .. } }) => {
                let text = lit_text(v).unwrap_or_default();
                let key = (text, 0);
                let name = match self.strings.get(&key) {
                    Some(n) => n.clone(),
                    None => {
                        let n = self.slot(e.clone());
                        self.strings.insert(key, n.clone());
                        n
                    }
                };
                const_read(span, name)
            }
            Val::Lit(e) => Expr { span, ..e.clone() },
            Val::Slot { name, .. } => const_read(span, name.clone()),
        }
    }
}

fn source_text<'s>(span: Span, sources: &'s [SourceFile]) -> Option<&'s [u8]> {
    let source = span.file.0.checked_sub(1).and_then(|i| sources.get(i as usize))?;
    source.text.as_bytes().get(span.start as usize..span.end as usize)
}

/// The call as written has no splat, double splat or block argument,
/// which the IR would have flattened into ordinary arguments.
fn call_syntax_kept(call: &Expr, sources: &[SourceFile]) -> bool {
    let Some(text) = source_text(call.span, sources) else { return false };
    let parsed = super::prism::parse_silent(text);
    if parsed.errors().next().is_some() {
        return false;
    }
    struct Erased(bool);
    impl<'pr> ruby_prism::Visit<'pr> for Erased {
        fn visit_branch_node_enter(&mut self, node: ruby_prism::Node<'pr>) {
            self.0 |= node.as_splat_node().is_some() || node.as_assoc_splat_node().is_some() || node.as_block_argument_node().is_some();
        }
        // Not the literal block's body: the IR keeps a splat there as written (`Alternative.new(*words)`).
        fn visit_block_node(&mut self, _: &ruby_prism::BlockNode<'pr>) {}
    }
    let mut erased = Erased(false);
    ruby_prism::Visit::visit(&mut erased, &parsed.node());
    !erased.0
}

/// A `define_method` as written has no splat, nested `def`, `defined?`,
/// and its block takes plain parameters: what the IR would have erased
/// into a simpler, different definition. A splat in a rescue clause's
/// class list (`rescue Timeout::Error, *ERRORS => e`) is not an argument
/// splat: the IR keeps it as written, a `Splat` among the clause's
/// classes, so it erases nothing.
fn definition_syntax_kept(statement: &Expr, sources: &[SourceFile]) -> bool {
    let Some(text) = source_text(statement.span, sources) else { return false };
    let parsed = super::prism::parse_silent(text);
    if parsed.errors().next().is_some() {
        return false;
    }
    struct Erased(bool, crate::det::HashSet<usize>);
    impl<'pr> ruby_prism::Visit<'pr> for Erased {
        fn visit_rescue_node(&mut self, node: &ruby_prism::RescueNode<'pr>) {
            for e in node.exceptions().iter() {
                if e.as_splat_node().is_some() {
                    self.1.insert(e.location().start_offset());
                }
            }
            ruby_prism::visit_rescue_node(self, node);
        }

        fn visit_branch_node_enter(&mut self, node: ruby_prism::Node<'pr>) {
            if node.as_splat_node().is_some() && self.1.contains(&node.location().start_offset()) {
                return;
            }
            self.0 |= node.as_splat_node().is_some()
                || node.as_assoc_splat_node().is_some()
                || node.as_def_node().is_some()
                || node.as_defined_node().is_some()
                || node.as_multi_target_node().is_some()
                || node.as_keyword_rest_parameter_node().is_some()
                || node.as_rest_parameter_node().is_some()
                || node.as_block_parameter_node().is_some();
        }
    }
    let mut erased = Erased(false, Default::default());
    ruby_prism::Visit::visit(&mut erased, &parsed.node());
    !erased.0
}

/// A synthetic span, not the capture's: Rubydex indexed the source, which has no such constant, and the class's own constants answer it.
fn const_read(_span: Span, name: Symbol) -> Expr {
    let mut e = Expr::new(Span::synthetic(), ExprNode::Const { path: vec![name] });
    e.decisions |= crate::expr::GENERATED_CONST_REF;
    e
}

fn touches_class_state(st: &Expr) -> bool {
    match &*st.node {
        ExprNode::Assign { target: LValue::Ivar { .. }, .. } | ExprNode::OpAssign { target: LValue::Ivar { .. }, .. } => true,
        ExprNode::Send { recv: Some(r), .. } => matches!(&*r.node, ExprNode::Ivar { .. }),
        _ => false,
    }
}

/// A short name for a statement, for a report.
fn describe(st: &Expr) -> String {
    match &*st.node {
        ExprNode::Send { method, recv, .. } => match recv {
            Some(_) => format!("a call to `.{}`", method.as_str()),
            None => format!("a call to `{}`", method.as_str()),
        },
        ExprNode::Assign { target: LValue::Index { .. }, .. } => "an index write".to_string(),
        ExprNode::Assign { target: LValue::Attr { name, .. }, .. } => format!("a write of `{}`", name.as_str()),
        other => format!("a `{}`", other.kind_str()),
    }
}

/// ActiveSupport's `blank?` on a literal.
fn blank(e: &Expr) -> Option<bool> {
    Some(match &*e.node {
        ExprNode::Lit { value: Literal::Nil } => true,
        ExprNode::Lit { value: Literal::Bool { value } } => !value,
        ExprNode::Lit { value: Literal::Str { value } } => value.trim().is_empty(),
        ExprNode::Lit { .. } => false,
        ExprNode::Hash { entries, .. } => entries.is_empty(),
        ExprNode::Array { elements, .. } => elements.is_empty(),
        _ => return None,
    })
}

/// What a macro may declare on its caller, replayed in the caller's body.
const CLASS_BODY_DECLARATIONS: &[&str] = &[
    "cattr_accessor", "cattr_reader", "cattr_writer", "mattr_accessor", "mattr_reader", "mattr_writer", "class_attribute",
    "attribute", "validates", "validate", "validates_presence_of", "validates_numericality_of", "validates_uniqueness_of",
    "before_validation", "after_validation", "before_save", "after_save", "around_save", "before_create", "after_create",
    "before_update", "after_update", "before_destroy", "after_destroy", "after_commit", "after_touch",
    "scope", "has_many", "has_one", "belongs_to", "has_and_belongs_to_many", "delegate",
    "attr_accessor", "attr_reader", "attr_writer",
];

fn lit(span: Span, value: Literal) -> Expr {
    Expr::new(span, ExprNode::Lit { value })
}

fn attr_writer(def: &MethodDef) -> bool {
    writer_ivar(def).is_some()
}

fn writer_ivar(def: &MethodDef) -> Option<Symbol> {
    let body = match &*def.body.node {
        ExprNode::Seq { exprs } if exprs.len() == 1 => &exprs[0],
        _ => &def.body,
    };
    let [param] = def.params.as_slice() else { return None };
    match &*body.node {
        ExprNode::Assign { target: LValue::Ivar { name }, value } if matches!(&*value.node, ExprNode::Var { name: v, .. } if v == &param.name) => Some(name.clone()),
        _ => None,
    }
}

// A `gsub` pattern with a metacharacter is left alone: folding it would need Ruby's regex engine.
fn inflected(method: &str, text: &str, args: &[Expr], mut fold: impl FnMut(&Expr) -> Option<Val>) -> Option<String> {
    use crate::naming;
    let pluralize = |path: &str| match path.rsplit_once('/') {
        Some((ns, last)) => format!("{ns}/{}", naming::pluralize_snake(last)),
        None => naming::pluralize_snake(path),
    };
    Some(match (method, args) {
        ("camelize", []) => naming::camelize_path(text),
        ("underscore", []) => naming::underscore(text),
        ("tableize", []) => pluralize(&naming::underscore(text)),
        ("pluralize", []) => pluralize(text),
        ("singularize", []) => naming::singularize_last(text),
        ("classify", []) => naming::classify_path(text),
        ("demodulize", []) => naming::demodulize(text).to_string(),
        ("gsub", [pattern, replacement]) => {
            let ExprNode::Lit { value: Literal::Regex { pattern, flags } } = &*pattern.node else { return None };
            let plain: String = {
                let mut out = String::new();
                let mut chars = pattern.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => match chars.next() {
                            Some(n) if !n.is_ascii_alphanumeric() => out.push(n),
                            _ => return None,
                        },
                        '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' => return None,
                        c => out.push(c),
                    }
                }
                out
            };
            if !flags.is_empty() || plain.is_empty() {
                return None;
            }
            let Val::Lit(replacement) = fold(replacement)? else { return None };
            let ExprNode::Lit { value: Literal::Str { value: replacement } } = &*replacement.node else { return None };
            if replacement.contains('\\') {
                return None;
            }
            text.replace(&plain, replacement)
        }
        _ => return None,
    })
}

fn lit_text(v: &Val) -> Option<String> {
    let Val::Lit(e) = v else { return None };
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Int { value } } => Some(value.to_string()),
        _ => None,
    }
}

fn literal_eq(a: &Literal, b: &Literal) -> Option<bool> {
    Some(match (a, b) {
        (Literal::Str { value: x }, Literal::Str { value: y }) => x == y,
        (Literal::Sym { value: x }, Literal::Sym { value: y }) => x == y,
        (Literal::Int { value: x }, Literal::Int { value: y }) => x == y,
        (Literal::Nil, Literal::Nil) => true,
        (Literal::Bool { value: x }, Literal::Bool { value: y }) => x == y,
        (Literal::Str { .. } | Literal::Sym { .. } | Literal::Int { .. } | Literal::Nil | Literal::Bool { .. }, _) => false,
        _ => return None,
    })
}

fn plain_param(name: Symbol) -> Param {
    Param { name, default: None, keyword: false, rest: false, forwarding: false, from_keyword: false, from_kwrest: false }
}

fn collect_locals(e: &Expr, out: &mut HashSet<Symbol>) {
    visit(e, &mut |x| match &*x.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } => {
            out.insert(name.clone());
        }
        _ => {}
    });
}

fn assigns(e: &Expr, name: &Symbol) -> bool {
    let mut found = false;
    visit(e, &mut |x| {
        found |= matches!(&*x.node,
            ExprNode::Assign { target: LValue::Var { name: n, .. }, .. } | ExprNode::OpAssign { target: LValue::Var { name: n, .. }, .. } if n == name);
    });
    found
}

/// `__send__(:value_of, key)` / `send(:x)` on `self` with a literal
/// name is the direct call, which a private method also answers.
fn collapse_literal_send(e: &mut Expr) {
    let ExprNode::Send { recv, method, args, block, parenthesized } = &*e.node else { return };
    if !matches!(method.as_str(), "send" | "__send__") || recv.as_ref().is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef)) {
        return;
    }
    let Some(first) = args.first() else { return };
    let name = match &*first.node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.clone(),
        _ => return,
    };
    if !valid_def_name(&name) {
        return;
    }
    *e.node = ExprNode::Send { recv: None, method: name, args: args[1..].to_vec(), block: block.clone(), parenthesized: *parenthesized };
}

fn valid_def_name(name: &Symbol) -> bool {
    let header = format!("def {name}\nend\n");
    let parsed = super::prism::parse_silent(header.as_bytes());
    let node = parsed.node();
    let Some(program) = node.as_program_node() else { return false };
    let nodes: Vec<_> = program.statements().body().iter().collect();
    let [node] = nodes.as_slice() else { return false };
    let Some(def) = node.as_def_node() else { return false };
    parsed.errors().next().is_none() && def.receiver().is_none() && def.name().as_slice() == name.as_str().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_word_index_counts_what_the_scan_counts() {
        let texts = [
            "def foo; end\ndef self.foo; end\nfoo\nfoo()\nx.foo = 1\nfoo=1\nfoo == 2\nfoo?\nfoo!\nfoo?!\nfoo??\n",
            "foo? foo?\nfoo?=\nfoo!=\n a.foo?.bar foo!x foo?x\nundef foo\ndef  foo\nprivate def foo\n:foo :foo? :foo= :foo!\n",
            "_foo foo_ foo9 9foo Foo FOO é_foo fooé éfoo é foo é\n\"foo\" 'foo' foo.bar foo::Baz Baz::foo ::foo\n",
            "def\ndef \ndef self.\nself.foo\nfoo\nfoo=\n[]\n<=>\n== foo ==\n",
            "a\naa\na a\naaa aa a?\na=a=\n?\n!\n=\n_\n__\n1\n12\n",
            "",
            "foo",
            "foo?",
            "foo=",
        ];
        let sources: Vec<SourceFile> =
            texts.iter().enumerate().map(|(i, text)| SourceFile { path: format!("{i}.rb"), text: text.to_string() }).collect();
        let names = [
            "foo", "foo?", "foo!", "foo=", "foo??", "Foo", "FOO", "_foo", "foo_", "foo9", "9foo", "é_foo", "éfoo", "é", "bar", "baz",
            "a", "aa", "a?", "a=", "_", "__", "1", "12", "def", "self", "[]", "<=>", "==", "?", "!", "=", "", "foo bar", "foo.bar",
        ];
        let mentions = Mentions::new(&sources);
        for name in names {
            assert_eq!(mentions.count(name), mentions_scan(name, &sources), "{name:?}");
            assert_eq!(mentions.mentioned(name), mentions_scan(name, &sources) > 0, "{name:?}");
        }
        assert!(mentions.count("foo") > 0 && mentions.count("foo?") > 0 && mentions.count("a") > 0);
    }

    /// Many sources split into several chunks and counted on worker threads: the index
    /// holds the totals the one-pass scan gives, whatever the chunk boundaries.
    #[test]
    fn the_parallel_index_counts_what_the_scan_counts() {
        let sources: Vec<SourceFile> = (0..500)
            .map(|i| SourceFile {
                path: format!("{i}.rb"),
                text: format!("def run_{}\n  helper_{}(x)\n  shared? && flag!\n  self.shared = {i}\nend\nshared shared= [] ==\n", i % 7, i % 5),
            })
            .collect();
        let mentions = Mentions::new(&sources);
        for name in ["shared", "shared?", "shared=", "flag!", "helper_0", "helper_4", "run_3", "x", "end", "self", "[]", "==", "absent"] {
            assert_eq!(mentions.count(name), mentions_scan(name, &sources), "{name:?}");
        }
        assert_eq!(mentions.count("shared"), 1000);
    }

    /// A name the word index does not answer (an operator) is scanned once, however often
    /// it is asked: ingest asks `==` and `[]` per method, and scanning every source each time
    /// held a large app's ingest on one thread for ten minutes.
    #[test]
    fn a_scanned_name_is_scanned_once() {
        let sources = [SourceFile { path: "a.rb".to_string(), text: "a == b\nc[1]\n".to_string() }];
        let mentions = Mentions::new(&sources);
        for _ in 0..3 {
            assert_eq!(mentions.count("=="), 1);
            assert_eq!(mentions.count("[]"), 0);
        }
        assert_eq!(mentions.scans.borrow().len(), 2);
    }

    /// What `ancestors` and `class_method_table` were before `Named`: a scan
    /// of every model and library class per link of the chain.
    fn scanned_ancestors(app: &App, parents: &HashMap<ClassId, ClassId>, model: &ClassId) -> Vec<ClassId> {
        let mut out = Vec::new();
        let mut cur = parents.get(model);
        while let Some(p) = cur {
            if out.contains(p) || !(app.models.iter().any(|m| &m.name == p) || app.library_classes.iter().any(|lc| &lc.name == p)) {
                break;
            }
            out.push(p.clone());
            cur = parents.get(p);
        }
        out
    }

    fn scanned_class_method_table(app: &App, ancestors: &[ClassId]) -> HashMap<Symbol, MethodDef> {
        let mut table = HashMap::default();
        for id in ancestors.iter().rev() {
            let methods: Vec<&MethodDef> = match app.models.iter().find(|m| &m.name == id) {
                Some(model) => model.methods().collect(),
                None => app.library_classes.iter().filter(|lc| &lc.name == id).flat_map(|lc| lc.methods.iter()).collect(),
            };
            for m in methods.into_iter().filter(|m| m.receiver == MethodReceiver::Class) {
                table.insert(m.name.clone(), m.clone());
            }
        }
        table
    }

    /// Models and library classes with the awkward shapes: a name reopened
    /// in two files (its methods merge, the later winning), a library class
    /// that shares a model's name (the model is read, not the library
    /// class), a parent cycle, and a parent no class defines.
    fn app() -> App {
        let tree: crate::vfs::FileTree = [
            ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"things\" do |t|\n    t.string \"name\"\n  end\nend\n"),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  def self.shared = 1\nend\n"),
            ("app/models/thing.rb", "class Thing < ApplicationRecord\n  def self.own = 2\nend\n"),
            ("app/models/odd.rb", "class Odd < Vague\nend\n"),
        ]
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
        let mut app = crate::ingest::ingest_app_from_tree(tree).expect("ingest");
        for (file, src) in [
            ("lib/a.rb", "class Plain\n  def self.first = 1\nend\nclass Leaf < Plain\n  def self.leaf = 1\nend\n"),
            ("lib/b.rb", "class Plain\n  def self.second = 2\n  def self.first = 3\nend\nclass Thing\n  def self.shadowed = 4\nend\n"),
            ("lib/c.rb", "class Ping < Pong\n  def self.ping = 1\nend\nclass Pong < Ping\n  def self.pong = 1\nend\nclass Lost < Missing\nend\n"),
            ("lib/d.rb", "class Deep < Leaf\nend\nclass Deeper < Deep\nend\nclass Under < Thing\nend\n"),
        ] {
            app.library_classes.extend(crate::ingest::ingest_library_classes(src.as_bytes(), file).expect("library classes"));
        }
        app
    }

    #[test]
    fn named_lookups_match_the_scans_they_replaced() {
        let app = app();
        let parents: HashMap<ClassId, ClassId> = app
            .models
            .iter()
            .filter_map(|m| Some((m.name.clone(), m.parent.clone()?)))
            .chain(app.library_classes.iter().filter(|lc| !lc.is_module).filter_map(|lc| Some((lc.name.clone(), lc.parent.clone()?))))
            .collect();
        let named = Named::of(&app);
        let names: Vec<ClassId> =
            app.models.iter().map(|m| m.name.clone()).chain(app.library_classes.iter().map(|lc| lc.name.clone())).chain([ClassId(Symbol::from("Nobody"))]).collect();
        let mut longest = 0;
        for name in &names {
            let chain = ancestors(&named, &parents, name);
            assert_eq!(chain, scanned_ancestors(&app, &parents, name), "ancestors of {name:?}");
            longest = longest.max(chain.len());
            let mut lineage = vec![name.clone()];
            lineage.extend(chain);
            assert_eq!(class_method_table(&app, &named, &lineage), scanned_class_method_table(&app, &lineage), "class methods of {name:?}");
        }
        assert!(longest >= 3, "the fixture holds a chain of several links");
        let plain = class_method_table(&app, &named, &[ClassId(Symbol::from("Plain"))]);
        assert_eq!(plain.len(), 2, "a reopened class's methods merge: {:?}", plain.keys().collect::<Vec<_>>());
    }
}
