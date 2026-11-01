//! `rate_limit to: N, within: D, …` → a `before_action` and the private
//! method it runs.
//!
//! actionpack's `ActionController::RateLimiting` (7.2+; the Rails 8
//! authentication generator writes one on `SessionsController#create`
//! and one on `PasswordsController#create`, so the Rails Guides store
//! carries two) is one filter:
//!
//! ```ruby
//! rate_limit to: 10, within: 3.minutes, only: :create,
//!            with: -> { redirect_to new_session_path, alert: "Try again later." }
//! ```
//!
//! ```ruby
//! before_action -> { rate_limiting(to:, within:, by:, with:, store:, name:) }, **options
//!
//! def rate_limiting(to:, within:, by:, with:, store:, name:)
//!   cache_key = ["rate-limit", controller_path, name, instance_exec(&by)].compact.join(":")
//!   count = store.increment(cache_key, 1, expires_in: within)
//!   instance_exec(&with) if count && count > to
//! end
//! ```
//!
//! Filters are installed only after private-method synth succeeds, so a
//! failed re-ingest never leaves orphan `before_action`s (same collect →
//! synth → install shape as `invisible_captcha`).
//!
//! Generated here as a real method and a real filter, the way
//! `allow_browser` is, because every consumer downstream reads methods
//! and filters and none reads a class-body macro:
//!
//! ```ruby
//! def rate_limit_create
//!   if ActionController::RateLimiter.exceeded?(self.class.cache_store, "rate-limit:sessions:" + (request.remote_ip).to_s, 180, 10)
//!     redirect_to new_session_path, alert: "Try again later."
//!   end
//! end
//! ```
//!
//! plus `before_action :rate_limit_create` with the macro's `only:` /
//! `except:`. The counter and its window live in the shared runtime
//! (`ActionController::RateLimiter`, calling `increment` on the
//! controller's class-level `cache_store`, Rails' default `store:`).
//!
//! WHAT IS FOLDED AT INGEST. `within:` written as `<int>.<unit>` becomes
//! the seconds literal, so the generated method carries no Duration;
//! any other expression is forwarded through `.to_i`. `by:` and `with:`
//! accept `->` lambdas — their bodies are re-rendered to source and
//! ingested inside the method — and `with:` also accepts a method symbol.
//! They default to `request.remote_ip` and
//! `head :too_many_requests`, actionpack's own defaults. `controller_path`
//! is the class name underscored, which is what Rails derives too.
//!
//! An explicit `store:` is captured in a controller constant at class
//! load, not evaluated on each request. The filter reads the declaring
//! controller's constant even when inherited by a child controller.
//!
//! WHAT IS LEFT. A `by:` that is not a lambda, a `with:` that is neither
//! a parameterless lambda nor a directly callable method symbol, a store
//! expression depending on class-body locals or instance variables, a
//! computed scope, or a `name:` that is not a literal — the macro stays
//! where it was and the survey names it, which is the contract for a
//! class-body macro roundhouse cannot expand (`report_unrecognized_controller_macros`).
//! The method reaches the ruby-family lanes fully; a strict target
//! carries the `RateLimiter` call as a runtime seam, the posture
//! `allow_browser`'s concern form already has.

use crate::dialect::{Controller, ControllerBodyItem, LibraryClass};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;

use super::controller_macro_synth::{
    append_private_actions, expr_symbol_list, install_before_filter,
};

#[derive(Clone)]
struct Limit {
    method: String,
    to_src: String,
    within_src: String,
    key_src: String,
    with_src: String,
    store: Option<Expr>,
    store_src: String,
    store_constant: Option<Symbol>,
    only: Vec<Symbol>,
    except: Vec<Symbol>,
    if_cond: Option<Symbol>,
    unless_cond: Option<Symbol>,
    if_cond_expr: Option<Expr>,
    unless_cond_expr: Option<Expr>,
}

/// A boot constant is admitted only from a file consisting of its one
/// declaration. Other writes are remembered so a conditional/duplicate
/// declaration cannot accidentally look like a unique static store.
pub(super) struct InitializerStore {
    name: Symbol,
    value: Option<Expr>,
}

pub(super) fn initializer_stores(source: &[u8], file: &str) -> Vec<InitializerStore> {
    use super::util::{constant_id_str, flatten_statements};
    use ruby_prism::Visit;
    let parsed = ruby_prism::parse(source);
    let root = parsed.node();
    let Some(program) = root.as_program_node() else { return Vec::new() };
    let statements = flatten_statements(program.statements().as_node());
    struct Writes<'a> {
        file: &'a str,
        direct: Option<(u32, u32)>,
        stores: Vec<InitializerStore>,
    }
    impl Writes<'_> {
        fn blocked(&mut self, name: Option<ruby_prism::ConstantId<'_>>) {
            if let Some(name) = name {
                self.stores.push(InitializerStore {
                    name: Symbol::from(constant_id_str(&name)), value: None,
                });
            }
        }
    }
    // Walk nested scopes too: an explicitly global target can be written
    // from a class/module body or a setup method. Ambiguous lexical writes
    // conservatively block that name rather than proving a false sole writer.
    impl<'pr> Visit<'pr> for Writes<'_> {
        fn visit_constant_write_node(&mut self, write: &ruby_prism::ConstantWriteNode<'pr>) {
            let loc = write.location();
            let value = if self.direct == Some(super::prism_api::span_offsets(&loc)) {
                let (value, _) = super::prism::scope(|| super::survey::without_recording(|| super::ingest_expr(&write.value(), self.file)));
                value.ok().filter(boot_store_expression)
            } else { None };
            self.stores.push(InitializerStore { name: Symbol::from(constant_id_str(&write.name())), value });
            ruby_prism::visit_constant_write_node(self, write);
        }
        fn visit_constant_and_write_node(&mut self, node: &ruby_prism::ConstantAndWriteNode<'pr>) {
            self.blocked(Some(node.name()));
            ruby_prism::visit_constant_and_write_node(self, node);
        }
        fn visit_constant_or_write_node(&mut self, node: &ruby_prism::ConstantOrWriteNode<'pr>) {
            self.blocked(Some(node.name()));
            ruby_prism::visit_constant_or_write_node(self, node);
        }
        fn visit_constant_operator_write_node(&mut self, node: &ruby_prism::ConstantOperatorWriteNode<'pr>) {
            self.blocked(Some(node.name()));
            ruby_prism::visit_constant_operator_write_node(self, node);
        }
        fn visit_constant_path_write_node(&mut self, node: &ruby_prism::ConstantPathWriteNode<'pr>) {
            self.blocked(node.target().name());
            ruby_prism::visit_constant_path_write_node(self, node);
        }
        fn visit_constant_path_and_write_node(&mut self, node: &ruby_prism::ConstantPathAndWriteNode<'pr>) {
            self.blocked(node.target().name());
            ruby_prism::visit_constant_path_and_write_node(self, node);
        }
        fn visit_constant_path_or_write_node(&mut self, node: &ruby_prism::ConstantPathOrWriteNode<'pr>) {
            self.blocked(node.target().name());
            ruby_prism::visit_constant_path_or_write_node(self, node);
        }
        fn visit_constant_path_operator_write_node(&mut self, node: &ruby_prism::ConstantPathOperatorWriteNode<'pr>) {
            self.blocked(node.target().name());
            ruby_prism::visit_constant_path_operator_write_node(self, node);
        }
        fn visit_constant_target_node(&mut self, node: &ruby_prism::ConstantTargetNode<'pr>) {
            self.blocked(Some(node.name()));
        }
        fn visit_constant_path_target_node(&mut self, node: &ruby_prism::ConstantPathTargetNode<'pr>) {
            self.blocked(node.name());
            ruby_prism::visit_constant_path_target_node(self, node);
        }
    }
    let direct = if let [statement] = statements.as_slice() {
        let loc = statement.location();
        Some(super::prism_api::span_offsets(&loc))
    } else { None };
    let mut writes = Writes { file, direct, stores: Vec::new() };
    writes.visit(&root);
    writes.stores
}

pub(super) fn lower_rate_limit(app: &mut crate::App, blocked: std::collections::BTreeSet<Symbol>, resolver: &crate::analyze::ConstResolver) {
    let boot_constants: std::collections::BTreeSet<Symbol> = app.library_classes.iter()
        .filter(|class| class.name.0.as_str() == "Object")
        .flat_map(|class| class.constants.iter().map(|(name, _)| name.clone())).collect();
    let contexts = store_contexts(app);
    let bindings = store_bindings(app);
    let boot_values: std::collections::BTreeMap<_, _> = app.library_classes.iter()
        .filter(|class| class.name.0.as_str() == "Object")
        .flat_map(|class| class.constants.iter().cloned()).collect();
    for controller in &mut app.controllers {
        let local = &contexts[&controller.name].constants;
        let limits: Vec<_> = collect_limits(controller).into_iter().filter_map(|(idx, mut limit)| {
            if let Some(value) = limit.store.as_mut() {
                if !resolve_store_receiver(value, resolver, &bindings) { return None; }
                if depends_on_discarded_configuration(value, &controller.name, &contexts, &boot_values,
                    &mut std::collections::BTreeSet::new(), resolver, &bindings) { return None; }
                if let Some(StoreReference { name, global }) = store_reference(value) {
                    if global || !local.contains_key(&name) {
                        if blocked.contains(&name) || (global && !boot_constants.contains(&name)) { return None; }
                        if boot_constants.contains(&name) {
                            // Name the captured boot binding explicitly. Spinel's
                            // class-body lookup does not search Object implicitly.
                            value.node = Box::new(ExprNode::Const {
                                path: vec![Symbol::from("::Object"), name],
                            });
                        }
                    }
                }
            }
            Some((idx, limit))
        }).collect();
        if limits.is_empty() {
            continue;
        }
        let mut methods = String::new();
        for (_, l) in &limits {
            methods.push_str(&method_source(l));
        }
        if !append_private_actions(controller, "<rate_limit>", &methods) {
            continue;
        }
        // Indices are still valid: append only pushed at the end.
        for (idx, l) in limits.iter().rev() {
            if let Some(item) = controller.body.get_mut(*idx) {
                install_before_filter(
                    item,
                    &l.method,
                    l.only.clone(),
                    l.except.clone(),
                    false,
                    l.if_cond.clone(),
                    l.unless_cond.clone(),
                    l.if_cond_expr.clone(),
                    l.unless_cond_expr.clone(),
                );
            }
            if let (Some(value), Some(name)) = (&l.store, &l.store_constant) {
                controller.body.insert(*idx, ControllerBodyItem::Unknown {
                    expr: Expr::new(value.span, ExprNode::Assign {
                        target: LValue::Const { path: vec![name.clone()] },
                        value: value.clone(),
                    }),
                    leading_comments: Vec::new(),
                    leading_blank_line: false,
                });
            }
        }
    }
}

/// A lexical NAME can be shadowed by the controller or its ancestors.
/// Only rooted ::NAME and ::Object::NAME unconditionally select the boot
/// binding. Resolve an unrooted Object receiver before classifying it.
struct StoreReference {
    name: Symbol,
    global: bool,
}

fn constant_segments(path: &[Symbol]) -> (bool, Vec<&str>) {
    let mut parts: Vec<_> = path.iter().map(Symbol::as_str).collect();
    let mut rooted = false;
    if parts.first().is_some_and(|head| head.is_empty()) {
        rooted = true;
        parts.remove(0);
    } else if let Some(head) = parts.first_mut() {
        if let Some(bare) = head.strip_prefix("::") { rooted = true; *head = bare; }
    }
    (rooted, parts)
}

fn store_reference(expr: &Expr) -> Option<StoreReference> {
    let ExprNode::Const { path } = &*expr.node else { return None };
    let (rooted, parts) = constant_segments(path);
    match parts.as_slice() {
        [name] => Some(StoreReference { name: Symbol::from(*name), global: rooted }),
        ["Object", name] if rooted => Some(StoreReference { name: Symbol::from(*name), global: true }),
        _ => None,
    }
}

// Resolve bare constants in their written lexical nesting, before deciding
// whether they select a boot binding. The class's qualified name alone
// cannot distinguish `module A; class B` from `class A::B`.
// Object is itself lexical too: resolve its receiver before using it as
// the global Object. An unresolved binding keeps the macro's gap.
fn resolve_store_receiver(expr: &mut Expr, resolver: &crate::analyze::ConstResolver, bindings: &StoreBindings) -> bool {
    if let ExprNode::Const { path } = &*expr.node {
        let (rooted, parts) = constant_segments(path);
        if !rooted && parts.len() == 1 {
            let Some(name) = resolver.declaration_name(expr.span, path) else { return false };
            let mut resolved = vec![Symbol::from("")];
            resolved.extend(name.trim_start_matches("::").split("::").map(Symbol::from));
            expr.node = Box::new(ExprNode::Const { path: resolved });
        } else if !rooted && parts.len() > 1 && parts[0] == "Object" {
            let receiver_span = crate::span::Span {
                end: expr.span.start + "Object".len() as u32, ..expr.span
            };
            let receiver = Expr::new(receiver_span, ExprNode::Const { path: vec![Symbol::from("Object")] });
            let Some(class) = store_namespace(&receiver, resolver, bindings, &mut std::collections::BTreeSet::new()) else {
                return false;
            };
            let mut resolved = vec![Symbol::from("")];
            resolved.extend(class.0.as_str().split("::").map(Symbol::from));
            resolved.extend(parts[1..].iter().map(|part| Symbol::from(*part)));
            expr.node = Box::new(ExprNode::Const { path: resolved });
        }
    }
    let mut resolved = true;
    expr.node.for_each_child_mut(&mut |child| resolved &= resolve_store_receiver(child, resolver, bindings));
    resolved
}

type StoreBindings = std::collections::BTreeMap<String, Vec<Expr>>;

// Namespace aliases are value declarations in the resolver. Follow only
// a unique constant-to-constant binding whose source resolves to a class
// or module; computed receivers and cycles retain the macro's gap.
fn store_namespace(
    expr: &Expr,
    resolver: &crate::analyze::ConstResolver,
    bindings: &StoreBindings,
    seen: &mut std::collections::BTreeSet<String>,
) -> Option<crate::ident::ClassId> {
    let ExprNode::Const { path } = &*expr.node else { return None };
    if let Some(class) = resolver.namespace(expr.span, path) { return Some(class.clone()); }
    let name = resolver.declaration_name(expr.span, path)?;
    if !seen.insert(name.to_string()) { return None; }
    let [value] = bindings.get(name)?.as_slice() else { return None };
    store_namespace(value, resolver, bindings, seen)
}

fn store_bindings(app: &crate::App) -> StoreBindings {
    let mut bindings = StoreBindings::new();
    for class in &app.library_classes {
        for (name, value) in &class.constants {
            bindings.entry(format!("{}::{}", class.name.0.as_str(), name.as_str())).or_default().push(value.clone());
        }
    }
    for controller in &app.controllers {
        for item in &controller.body {
            if let ControllerBodyItem::Unknown { expr, .. } = item {
                if let ExprNode::Assign { target: LValue::Const { path }, value } = &*expr.node {
                    if let [name] = path.as_slice() {
                        bindings.entry(format!("{}::{}", controller.name.0.as_str(), name.as_str())).or_default().push(value.clone());
                    }
                }
            }
        }
    }
    bindings
}

#[derive(Default)]
struct StoreContext {
    constants: std::collections::BTreeMap<Symbol, Vec<Expr>>,
    discarded_cache_store: bool,
}

fn writes_cache_store(expr: &Expr) -> bool {
    let here = match &*expr.node {
        ExprNode::Assign { target: LValue::Attr { recv, name }, .. }
        | ExprNode::OpAssign { target: LValue::Attr { recv, name }, .. } =>
            name.as_str() == "cache_store" && matches!(&*recv.node, ExprNode::SelfRef),
        ExprNode::Send { recv: Some(recv), method, .. } =>
            method.as_str() == "cache_store=" && matches!(&*recv.node, ExprNode::SelfRef),
        _ => false,
    };
    let mut found = here;
    expr.node.for_each_child(&mut |child| found |= writes_cache_store(child));
    found
}

fn store_contexts(app: &crate::App) -> std::collections::BTreeMap<crate::ident::ClassId, StoreContext> {
    app.controllers.iter().map(|controller| {
        let mut context = StoreContext::default();
        let mut seen = std::collections::BTreeSet::new();
        let mut owner = Some(controller.name.clone());
        while let Some(name) = owner.take() {
            if !seen.insert(name.clone()) { break; }
            if let Some(class) = app.controllers.iter().find(|class| class.name == name) {
                let mut local: std::collections::BTreeMap<Symbol, Vec<Expr>> = std::collections::BTreeMap::new();
                for item in &class.body {
                    if let ControllerBodyItem::Unknown { expr, .. } = item {
                        context.discarded_cache_store |= writes_cache_store(expr);
                        if let ExprNode::Assign { target: LValue::Const { path }, value } = &*expr.node {
                            if let [name] = path.as_slice() {
                                local.entry(name.clone()).or_default().push(value.clone());
                            }
                        }
                    }
                }
                for (name, values) in local { context.constants.entry(name).or_insert(values); }
                owner = class.parent.clone();
            } else if let Some(class) = app.library_classes.iter().find(|class| class.name == name) {
                if class.name.0.as_str() == "Object" { break; }
                let mut local: std::collections::BTreeMap<Symbol, Vec<Expr>> = std::collections::BTreeMap::new();
                for (name, value) in &class.constants {
                    local.entry(name.clone()).or_default().push(value.clone());
                }
                for (name, values) in local { context.constants.entry(name).or_insert(values); }
                context.discarded_cache_store |= class.unknown_calls.iter().any(writes_cache_store);
                owner = class.parent.clone();
            }
        }
        (controller.name.clone(), context)
    }).collect()
}

// Controller lowering does not carry self.cache_store= statements. A
// self/bare factory call (or a constant derived from it) cannot safely
// capture that configuration until those statements survive in order.
fn depends_on_discarded_configuration(
    expr: &Expr,
    owner: &crate::ident::ClassId,
    contexts: &std::collections::BTreeMap<crate::ident::ClassId, StoreContext>,
    boot: &std::collections::BTreeMap<Symbol, Expr>,
    seen: &mut std::collections::BTreeSet<String>,
    resolver: &crate::analyze::ConstResolver,
    bindings: &StoreBindings,
) -> bool {
    let mut resolved = expr.clone();
    if !resolve_store_receiver(&mut resolved, resolver, bindings) { return true; }
    let expr = &resolved;
    let context = contexts.get(owner);
    if let Some(reference) = store_reference(expr) {
        let local = (!reference.global).then(|| context.and_then(|c| c.constants.get(&reference.name).map(Vec::as_slice))).flatten();
        let (key, value) = if let Some(value) = local {
            (format!("{}::{}", owner.0.as_str(), reference.name.as_str()), Some(value))
        } else {
            (format!("Object::{}", reference.name.as_str()), boot.get(&reference.name).map(std::slice::from_ref))
        };
        if let Some(values) = value {
            if !seen.insert(key.clone()) { return true; }
            let depends = values.iter().any(|value| depends_on_discarded_configuration(value, owner, contexts, boot, seen, resolver, bindings));
            seen.remove(&key);
            return depends;
        }
    }
    let self_configured = context.is_some_and(|c| c.discarded_cache_store);
    match &*expr.node {
        ExprNode::SelfRef if self_configured => return true,
        ExprNode::Send { recv: None, .. } if self_configured => return true,
        ExprNode::Send { recv: Some(recv), .. } => {
            if let ExprNode::Const { path } = &*recv.node {
                let (_, parts) = constant_segments(path);
                let class = crate::ident::ClassId(Symbol::from(parts.join("::")));
                if contexts.get(&class).is_some_and(|c| c.discarded_cache_store) { return true; }
            }
        }
        ExprNode::Const { path } => {
            let (_, parts) = constant_segments(path);
            if let Some((name, prefix)) = parts.split_last() {
                let class = crate::ident::ClassId(Symbol::from(prefix.join("::")));
                let key = parts.join("::");
                if let Some(values) = contexts.get(&class).and_then(|c| c.constants.get(&Symbol::from(*name)))
                    .or_else(|| bindings.get(&key)) {
                    if !seen.insert(key.clone()) { return true; }
                    let depends = values.iter().any(|value| depends_on_discarded_configuration(value, &class, contexts, boot, seen, resolver, bindings));
                    seen.remove(&key);
                    return depends;
                }
            }
        }
        _ => {}
    }
    let mut depends = false;
    expr.node.for_each_child(&mut |child| {
        depends |= depends_on_discarded_configuration(child, owner, contexts, boot, seen, resolver, bindings);
    });
    depends
}

/// Ruby puts a file-level constant on Object. Carry only boot stores that
/// rate_limit actually references, keeping one object shared by controllers.
/// A conditional, duplicate or compound boot file retains the macro's gap.
pub(super) fn install_boot_stores(app: &mut crate::App, stores: Vec<InitializerStore>, resolver: &crate::analyze::ConstResolver) -> (std::collections::BTreeSet<Symbol>, Vec<crate::span::Span>) {
    let mut blocked = std::collections::BTreeSet::new();
    let mut constants = Vec::new();
    let contexts = store_contexts(app);
    let bindings = store_bindings(app);
    let bindings = &bindings;
    let used: std::collections::BTreeSet<Symbol> = app.controllers.iter().flat_map(|controller| {
        let context = &contexts[&controller.name];
        collect_limits(controller).into_iter().filter_map(move |(_, limit)| {
            let mut store = limit.store?;
            if !resolve_store_receiver(&mut store, resolver, bindings) { return None; }
            store_reference(&store)
                .filter(|reference| reference.global || !context.constants.contains_key(&reference.name))
                .map(|reference| reference.name)
        })
    }).collect();
    let mut counts = std::collections::BTreeMap::new();
    for store in &stores { *counts.entry(store.name.clone()).or_insert(0) += 1; }
    for store in &stores {
        blocked.insert(store.name.clone());
        if !used.contains(&store.name) { continue; }
        if counts[&store.name] != 1 { continue; }
        if let Some(value) = &store.value {
            constants.push((store.name.clone(), value.clone()));
            blocked.remove(&store.name);
        }
    }
    let mut carried = Vec::new();
    if !constants.is_empty() {
        if let Some(object) = app.library_classes.iter_mut().find(|c| c.name.0.as_str() == "Object") {
            for (name, value) in constants {
                if object.constants.iter().any(|(existing, _)| existing == &name) {
                    blocked.insert(name);
                } else {
                    carried.push(value.span);
                    object.constants.push((name, value));
                }
            }
        } else {
            carried.extend(constants.iter().map(|(_, value)| value.span));
            app.library_classes.push(LibraryClass {
                name: crate::ident::ClassId(Symbol::from("Object")),
                is_module: false, parent: None, parent_span: crate::span::Span::synthetic(),
                includes: Vec::new(), include_spans: Vec::new(), methods: Vec::new(),
                nullable_columns: Vec::new(), origin: None, constants,
                conditional_definitions: Vec::new(), unknown_calls: Vec::new(), class_ivar_initializers: Vec::new(),
            });
        }
    }
    (blocked, carried)
}

// The initializer's self is Ruby's main object, not the controller or
// Object's class object. Only explicit receivers survive that move.
fn boot_store_expression(value: &Expr) -> bool {
    fn literal(expr: &Expr) -> bool {
        match &*expr.node {
            ExprNode::Lit { .. } => true,
            ExprNode::Array { elements, .. } => elements.iter().all(literal),
            ExprNode::Hash { entries, .. } => entries.iter().all(|(k, v)| literal(k) && literal(v)),
            _ => false,
        }
    }
    match &*value.node {
        ExprNode::Send { recv: Some(recv), args, block: None, .. } => {
            matches!(&*recv.node, ExprNode::Const { .. }) && args.iter().all(literal)
        }
        ExprNode::Const { path } => {
            let parts: Vec<_> = path.iter().filter(|p| !p.as_str().is_empty()).collect();
            // An alias to another boot constant would need its own dependency
            // closure. A class/module's constant is carried with that owner.
            parts.len() > 1 && parts[0].as_str() != "Object"
        }
        _ => false,
    }
}

/// ```ruby
///   def rate_limit_create
///     if ActionController::RateLimiter.exceeded?(self.class.cache_store, <key>, <within>, <to>)
///       <with>
///     end
///   end
/// ```
fn method_source(l: &Limit) -> String {
    format!(
        "  def {}\n    if ActionController::RateLimiter.exceeded?({}, {}, {}, {})\n      {}\n    end\n  end\n",
        l.method, l.store_src, l.key_src, l.within_src, l.to_src, l.with_src
    )
}

/// Parse expandable `rate_limit` calls without mutating the body.
fn collect_limits(controller: &Controller) -> Vec<(usize, Limit)> {
    let path = crate::naming::underscore(
        controller.name.0.as_str().strip_suffix("Controller").unwrap_or(controller.name.0.as_str()),
    );
    let mut found: Vec<(usize, Limit)> = Vec::new();
    let mut methods: std::collections::BTreeSet<String> = controller.actions()
        .map(|a| a.name.as_str().to_string()).collect();
    let mut constants: std::collections::BTreeSet<Symbol> = controller.body.iter()
        .filter_map(|item| match item {
            ControllerBodyItem::Unknown { expr, .. } => match &*expr.node {
                ExprNode::Assign { target: LValue::Const { path }, .. } => path.last().cloned(),
                _ => None,
            },
            _ => None,
        }).collect();
    for (i, item) in controller.body.iter().enumerate() {
        let ControllerBodyItem::Unknown { expr, .. } = item else {
            continue;
        };
        let Some(mut limit) = limit_from_call(expr, &path) else {
            continue;
        };
        // Two macros on one controller (or one without a `name:`) must
        // not collide on the method name; the ordinal keeps them apart.
        let stem = limit.method.clone();
        let mut ordinal = 2;
        while !methods.insert(limit.method.clone()) {
            limit.method = format!("{stem}_{ordinal}");
            ordinal += 1;
        }
        if limit.store.is_some() {
            let mut ordinal = found.len() + 1;
            let mut constant = Symbol::from(format!("RH_RATE_LIMIT_STORE_{ordinal}"));
            while !constants.insert(constant.clone()) {
                ordinal += 1;
                constant = Symbol::from(format!("RH_RATE_LIMIT_STORE_{ordinal}"));
            }
            // Qualify the owner: self.class would select a child's own
            // capture when an inherited filter runs on that child.
            limit.store_src = format!("{}::{}", controller.name.0.as_str(), constant.as_str());
            limit.store_constant = Some(constant);
        }
        found.push((i, limit));
    }
    found
}

/// `rate_limit to: N, within: D, by: -> {…}, with: -> {…}, name: "…",
/// scope: …, only: […], except: […]` → its parts, or None for any call this is
/// not, or an option it cannot expand.
fn limit_from_call(call: &Expr, controller_path: &str) -> Option<Limit> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else {
        return None;
    };
    if method.as_str() != "rate_limit" {
        return None;
    }
    let [opts] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, kwargs: true } = &*opts.node else { return None };
    let mut to_src: Option<String> = None;
    let mut within_src: Option<String> = None;
    let mut by_src: Option<String> = None;
    let mut with_src: Option<String> = None;
    let mut name: Option<String> = None;
    let mut scope_src: Option<String> = None;
    let mut store = None;
    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    let mut if_cond = None;
    let mut unless_cond = None;
    let mut if_cond_expr = None;
    let mut unless_cond_expr = None;
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
        match key.as_str() {
            "to" => to_src = Some(crate::emit::ruby::expr::emit_expr(v)),
            "within" => within_src = Some(seconds_source(v)),
            "by" => by_src = Some(lambda_body_source(v)?),
            "with" => with_src = Some(handler_source(v)?),
            "name" => {
                let ExprNode::Lit { value: Literal::Str { value } } = &*v.node else { return None };
                name = Some(value.clone());
            }
            // Rails defaults the cache-key scope to this controller's
            // path. `scope:` replaces that, so two controllers can share
            // one budget: a symbol, a string, or a call such as
            // `OtherController.controller_path`.
            "scope" => scope_src = Some(scope_source(v)?),
            "only" => only = expr_symbol_list(v)?,
            "except" => except = expr_symbol_list(v)?,
            "if" => match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value } } => if_cond = Some(value.clone()),
                // The guard evaluates the predicate. Storing the lambda
                // would test the lambda object, which is always truthy.
                // A parameter has no binding once the body is inlined.
                ExprNode::Lambda { params, rest_param, extra_params, block_param, body, .. }
                    if params.is_empty() && rest_param.is_none() && extra_params.is_empty() && block_param.is_none() =>
                {
                    if_cond_expr = Some((*body).clone())
                }
                _ => return None,
            },
            "unless" => match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value } } => unless_cond = Some(value.clone()),
                ExprNode::Lambda { params, rest_param, extra_params, block_param, body, .. }
                    if params.is_empty() && rest_param.is_none() && extra_params.is_empty() && block_param.is_none() =>
                {
                    unless_cond_expr = Some((*body).clone())
                }
                _ => return None,
            },
            "store" => {
                if !store_expression(v) {
                    return None;
                }
                store = Some(v.clone());
            }
            _ => return None,
        }
    }
    let method = match (&name, only.as_slice()) {
        (Some(n), _) => format!("rate_limit_{}", method_suffix(&crate::naming::snake_case(n))),
        (None, [one]) => format!("rate_limit_{}", method_suffix(one.as_str())),
        (None, _) => "rate_limit".to_string(),
    };
    // Rails: ["rate-limit", scope || controller_path, name, by].compact.join(":")
    let key_src = match scope_src {
        Some(scope) => {
            let mut parts = vec![format!(
                "{} + ({}).to_s",
                ruby_string_literal("rate-limit:"),
                scope
            )];
            if let Some(n) = &name {
                parts.push(ruby_string_literal(&format!(":{n}:")));
            } else {
                parts.push(ruby_string_literal(":"));
            }
            parts.push(format!(
                "({}).to_s",
                by_src.unwrap_or_else(|| "request.remote_ip".to_string())
            ));
            parts.join(" + ")
        }
        None => {
            let mut prefix = format!("rate-limit:{controller_path}");
            if let Some(n) = &name {
                prefix.push(':');
                prefix.push_str(n);
            }
            prefix.push(':');
            format!(
                "{} + ({}).to_s",
                ruby_string_literal(&prefix),
                by_src.unwrap_or_else(|| "request.remote_ip".to_string())
            )
        }
    };
    Some(Limit {
        method,
        to_src: to_src?,
        within_src: within_src?,
        key_src,
        with_src: with_src.unwrap_or_else(|| "head(:too_many_requests)".to_string()),
        store,
        store_src: "self.class.cache_store".to_string(),
        store_constant: None,
        only,
        except,
        if_cond,
        unless_cond,
        if_cond_expr,
        unless_cond_expr,
    })
}

/// Names are cache-key data, not Ruby syntax. Keep ordinary generated
/// names stable, and allocate collisions after replacing punctuation.
fn method_suffix(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

fn handler_source(v: &Expr) -> Option<String> {
    if let ExprNode::Lit { value: Literal::Sym { value } } = &*v.node {
        let name = value.as_str();
        let stem = name.strip_suffix(['?', '!']).unwrap_or(name);
        let mut chars = stem.chars();
        if !chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return None;
        }
        // Explicit self also permits keyword spellings, and Ruby allows
        // private methods with the literal self receiver.
        return Some(format!("self.{name}()"));
    }
    lambda_body_source(v)
}

/// These expressions survive as class-load constant initializers. A local
/// or class-instance-variable read would depend on a class-body statement
/// that controller lowering does not carry, so retain the macro's gap.
fn store_expression(v: &Expr) -> bool {
    match &*v.node {
        ExprNode::Const { .. } | ExprNode::SelfRef | ExprNode::Lit { .. } => true,
        ExprNode::Send { recv, args, block: None, .. } => {
            recv.as_ref().is_none_or(store_expression) && args.iter().all(store_expression)
        }
        ExprNode::Array { elements, .. } => elements.iter().all(store_expression),
        ExprNode::Hash { entries, .. } => entries.iter().all(|(k, v)| store_expression(k) && store_expression(v)),
        _ => false,
    }
}

/// `scope:` as the cache-key segment Rails joins. A symbol or string is
/// that segment; a call (`SessionsController.controller_path`) is
/// evaluated, which is how one controller names another's path.
fn scope_source(v: &Expr) -> Option<String> {
    // A symbol or string is captured when the macro expands, which is
    // when Rails evaluates `scope:`. A call is not: Rails would run it
    // once at declaration, and a nil or false result would fall back to
    // this controller's path. Emitting the call inside the request would
    // re-run it, and a nil result would collapse unrelated budgets.
    match &*v.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(ruby_string_literal(value.as_str())),
        ExprNode::Lit { value: Literal::Str { value } } => Some(ruby_string_literal(value)),
        _ => None,
    }
}

/// The body of a parameterless `-> { … }` as source; None for anything
/// else. Rails calls `by:` / `with:` without arguments, so a parameter
/// (a default such as `->(key = request.remote_ip) { key }` included)
/// has nothing to bind once only the body is inlined.
fn lambda_body_source(v: &Expr) -> Option<String> {
    let ExprNode::Lambda { params, rest_param, extra_params, block_param, body, .. } = &*v.node else {
        return None;
    };
    if !params.is_empty() || rest_param.is_some() || !extra_params.is_empty() || block_param.is_some() {
        return None;
    }
    Some(crate::emit::ruby::expr::emit_expr(body))
}

/// `within:` as seconds: `3.minutes` folds to `180`; anything else is
/// forwarded through `.to_i`, which reads seconds off an Integer and
/// off an `ActiveSupport::Duration` alike.
fn seconds_source(v: &Expr) -> String {
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*v.node {
        if args.is_empty() {
            if let ExprNode::Lit { value: Literal::Int { value: n } } = &*r.node {
                let per = match method.as_str() {
                    "second" | "seconds" => Some(1),
                    "minute" | "minutes" => Some(60),
                    "hour" | "hours" => Some(3600),
                    "day" | "days" => Some(86400),
                    "week" | "weeks" => Some(7 * 86400),
                    _ => None,
                };
                if let Some(per) = per {
                    return (n * per).to_string();
                }
            }
        }
    }
    format!("({}).to_i", crate::emit::ruby::expr::emit_expr(v))
}

pub(super) fn ruby_string_literal(s: &str) -> String {
    // A double-quoted literal interpolates `#{…}`, `#@ivar`, and
    // `#$global`. The scope was a literal, so each marker stays text.
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut out = String::with_capacity(escaped.len());
    let chars: Vec<char> = escaped.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' && matches!(chars.get(i + 1), Some('{' | '@' | '$')) {
            out.push('\\');
        }
        out.push(chars[i]);
        i += 1;
    }
    format!("\"{out}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(src: &str) -> Controller {
        super::super::controller::ingest_controller(src.as_bytes(), "app/controllers/sessions_controller.rb")
            .unwrap()
            .unwrap()
    }

    /// Collect then install filters (mirrors `lower_rate_limit` after synth).
    fn collect_and_install(c: &mut Controller) -> Vec<Limit> {
        let collected = collect_limits(c);
        for (idx, l) in &collected {
            if let Some(item) = c.body.get_mut(*idx) {
                install_before_filter(
                    item,
                    &l.method,
                    l.only.clone(),
                    l.except.clone(),
                    false,
                    l.if_cond.clone(),
                    l.unless_cond.clone(),
                    l.if_cond_expr.clone(),
                    l.unless_cond_expr.clone(),
                );
            }
        }
        collected.into_iter().map(|(_, l)| l).collect()
    }

    #[test]
    fn the_store_form_expands_to_a_filter_and_a_method() {
        let mut c = controller(
            "class SessionsController < ApplicationController\n  \
             rate_limit to: 10, within: 3.minutes, only: :create, with: -> { redirect_to new_session_path, alert: \"Try again later.\" }\n  \
             def create\n  end\nend\n",
        );
        let limits = collect_and_install(&mut c);
        assert_eq!(limits.len(), 1);
        let l = &limits[0];
        assert_eq!(l.method, "rate_limit_create");
        assert_eq!(l.to_src, "10");
        assert_eq!(l.within_src, "180");
        assert_eq!(l.key_src, "\"rate-limit:sessions:\" + (request.remote_ip).to_s");
        assert!(l.with_src.starts_with("redirect_to"), "{}", l.with_src);
        assert_eq!(l.only, vec![Symbol::from("create")]);
        let filter = c.body.iter().find_map(|i| match i {
            ControllerBodyItem::Filter { filter, .. } => Some(filter),
            _ => None,
        });
        assert_eq!(filter.unwrap().target.as_str(), "rate_limit_create");
        let src = method_source(l);
        assert!(src.contains("ActionController::RateLimiter.exceeded?(self.class.cache_store, \"rate-limit:sessions:\" + (request.remote_ip).to_s, 180, 10)"), "{src}");
    }

    #[test]
    fn defaults_and_the_key_name_follow_rails() {
        let c = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 5, within: 1.hour, by: -> { params[:email] }, name: \"signup\"\nend\n",
        );
        let limits = collect_limits(&c);
        let l = &limits[0].1;
        assert_eq!(l.method, "rate_limit_signup");
        assert_eq!(l.within_src, "3600");
        assert_eq!(l.key_src, "\"rate-limit:api/tokens:signup:\" + (params[:email]).to_s");
        assert_eq!(l.with_src, "head(:too_many_requests)");
        assert!(l.only.is_empty());
    }

    #[test]
    fn a_shared_scope_replaces_the_controller_path_in_the_cache_key() {
        // Two controllers count against one budget when the second names
        // the first's path, a symbol, or a string. Rails joins
        // `scope || controller_path` between `rate-limit` and `name`.
        // A call is evaluated by Rails when the filter is declared.
        // Expanding it into the request method would re-run it, and a
        // nil result would collapse this controller's budget into
        // another. The macro stays in place.
        let shared = controller(
            "class Users::Sessions::OtpsController < ApplicationController\n  \
             rate_limit to: 10, within: 3.minutes, only: :create, scope: Users::SessionsController.controller_path\nend\n",
        );
        assert!(collect_limits(&shared).is_empty());

        let symbol = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 100, within: 5.minutes, scope: :api_global\nend\n",
        );
        let symbol = &collect_limits(&symbol)[0].1;
        assert_eq!(
            symbol.key_src,
            "\"rate-limit:\" + (\"api_global\").to_s + \":\" + (request.remote_ip).to_s"
        );

        let named = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 100, within: 5.minutes, name: \"burst\", scope: \"api\"\nend\n",
        );
        let named = &collect_limits(&named)[0].1;
        assert!(named.key_src.contains("\"api\""), "{}", named.key_src);
        assert!(named.key_src.contains(":burst:"), "{}", named.key_src);

        let marked = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 100, within: 5.minutes, scope: \"budget-\\#{id}\"\nend\n",
        );
        let marked = &collect_limits(&marked)[0].1;
        assert!(
            marked.key_src.contains("\\#{"),
            "a literal interpolation marker stays literal; {}",
            marked.key_src
        );

        // Single quotes keep `#@` and `#$` as text. Emitting them inside
        // a double-quoted key would interpolate the ivar or the global.
        let shorthand = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 100, within: 5.minutes, scope: '#@token #$budget'\nend\n",
        );
        let shorthand = &collect_limits(&shorthand)[0].1;
        assert!(
            shorthand.key_src.contains("\\#@token") && shorthand.key_src.contains("\\#$budget"),
            "shorthand interpolation markers stay literal; {}",
            shorthand.key_src
        );
    }

    #[test]
    fn an_unknown_option_leaves_the_macro_in_place() {
        let c = controller(
            "class SessionsController < ApplicationController\n  \
             rate_limit to: 10, within: 3.minutes, bogus: true\nend\n",
        );
        assert!(collect_limits(&c).is_empty());
        assert!(c.body.iter().any(|i| matches!(i, ControllerBodyItem::Unknown { .. })));
    }

    #[test]
    fn if_unless_and_by_lambda_are_kept() {
        let mut c = controller(
            "class TokensController < ApplicationController\n  \
             rate_limit to: 5, within: 1.minute, only: :create, if: -> { @credential }, unless: :quiet?, by: -> { @token }\nend\n",
        );
        let limits = collect_and_install(&mut c);
        assert_eq!(limits.len(), 1);
        let limit = &limits[0];
        assert_eq!(limit.only, vec![Symbol::from("create")]);
        assert!(limit.if_cond_expr.is_some());
        assert_eq!(limit.unless_cond.as_ref().map(|s| s.as_str()), Some("quiet?"));
        assert!(limit.key_src.contains("@token"), "{}", limit.key_src);
        let filter = c.body.iter().find_map(|item| match item {
            ControllerBodyItem::Filter { filter, .. } => Some(filter),
            _ => None,
        }).expect("filter");
        assert!(filter.if_cond_expr.is_some());
        assert_eq!(filter.unless_cond.as_ref().map(|s| s.as_str()), Some("quiet?"));
    }

    #[test]
    fn a_custom_store_is_captured_on_its_declaring_controller() {
        let c = controller(
            "class SessionsController < ApplicationController\n  \
             rate_limit to: 10, within: 3.minutes, store: custom_store\nend\n",
        );
        let limits = collect_limits(&c);
        let l = &limits[0].1;
        assert!(l.store.is_some());
        assert_eq!(l.store_src, "SessionsController::RH_RATE_LIMIT_STORE_1");
        let src = method_source(l);
        assert!(src.contains("exceeded?(SessionsController::RH_RATE_LIMIT_STORE_1,"), "{src}");
        assert!(!src.contains("custom_store"), "the factory must not run per request: {src}");
    }

    #[test]
    fn a_symbol_handler_is_a_direct_instance_call() {
        let c = controller("class SessionsController < ApplicationController\nrate_limit to: 2, within: 60, with: :limited\nend");
        assert_eq!(collect_limits(&c)[0].1.with_src, "self.limited()");
        let c = controller("class SessionsController < ApplicationController\nrate_limit to: 2, within: 60, with: :'not-a-method'\nend");
        assert!(collect_limits(&c).is_empty());
    }

    #[test]
    fn unsupported_store_contexts_keep_the_declaration() {
        for store in ["@store", "-> { custom_store }", "stores.map { |s| s }"] {
            let c = controller(&format!("class SessionsController < ApplicationController\nrate_limit to: 2, within: 60, store: {store}\nend"));
            assert!(collect_limits(&c).is_empty(), "{store}");
            assert!(c.body.iter().any(|i| matches!(i, ControllerBodyItem::Unknown { .. })));
        }
    }

    #[test]
    fn literal_names_remain_data_and_generated_identifiers_do_not_collide() {
        let mut c = controller(r#"class SessionsController < ApplicationController
  rate_limit to: 2, within: 60, name: "short-term"
  rate_limit to: 5, within: 60, name: "short_term"
  rate_limit to: 8, within: 60, name: "short_term_2"
  rate_limit to: 10, within: 60, name: 'a / b!? 東京 #@literal'
  def rate_limit_short_term
  end
end"#);
        let limits = collect_and_install(&mut c);
        assert_eq!(limits.len(), 4);
        let mut names = std::collections::BTreeSet::new();
        for l in &limits {
            assert!(names.insert(&l.method));
            assert!(l.method.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        }
        assert_ne!(limits[0].method, "rate_limit_short_term");
        assert!(limits[0].key_src.contains(":short-term:"));
        assert!(limits[3].key_src.contains("東京 \\#@literal"));
        let methods = limits.iter().map(method_source).collect::<String>();
        assert!(append_private_actions(&mut c, "<rate_limit>", &methods));
    }
}
