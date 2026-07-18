//! Shared lowering: dynamic `send` → static `case` dispatch.
//!
//! Spinel AOT rejects `send` with a runtime method name ("AOT needs a
//! compile-time-known name"), and every strict target has the same
//! problem — reflective dispatch defeats whole-program resolution. But
//! the app-code idioms that reach `send` dynamically almost always draw
//! the method name from a literal collection in the same method (the
//! `as_json` spec-array walk, `[:q, :what, :order].map { |p| send(p) }`)
//! or from an enumerable helper table (lobsters' `IntervalHelper`). When
//! the pass can prove the full name set, it rewrites
//!
//!   js[k] = send(k)
//!
//! into
//!
//!   js[k] = case k
//!           when :short_id then short_id
//!           ...
//!           else raise "..."
//!           end
//!
//! which every target compiles and CRuby executes identically (the
//! `else` arm preserves `send`'s NoMethodError-on-unknown behavior in
//! spirit). Sites whose name set can't be proven go on the residue
//! ledger and keep their source shape — an incomplete arm list would
//! turn a legal dispatch into a raise, so enumeration bails toward
//! "no rewrite" on anything unclassifiable. This is the lowering-side
//! completion of the analyze-side receiver-aware send typing
//! (d776278): analyze bounds the *type*, this pass grounds the
//! *dispatch*.
//!
//! Runs on the post-analyze hook (`apply_post_analyze_lowerings`) so
//! every target consumes the grounded form. Two deltas vs the
//! emit-time ruby-family pass this re-homes: (a) shape C's provider
//! calls are matched in their SOURCE form — a bare `time_interval(...)`
//! reaching a mixed-in helper — where the emit pass saw them only
//! after helper-qualification (`IntervalHelper.time_interval(...)`);
//! (b) synthesized arms are ty-stamped from the analyzer's class
//! registry, because the residual-diagnostics audit walks hook output
//! (the emit-time pass ran after it and dodged).
//!
//! Duration-unit arms deliberately call the PLURAL unit form
//! (`"day" → days`) — identical semantics on a numeric receiver, and
//! the plural is what the shared duration lowering (running right
//! after this pass in the hook, by contract) grounds into the
//! Duration runtime unconditionally.

use std::collections::{BTreeSet, HashMap};

use crate::analyze::ClassInfo;
use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{ControllerBodyItem, ModelBodyItem};
use crate::expr::{Arm, Expr, ExprNode, Literal, Pattern};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

/// Method names that perform reflective dispatch on their first arg.
fn is_send_method(name: &str) -> bool {
    matches!(name, "send" | "public_send" | "__send__")
}

/// Ground dynamic `send` sites across every hook body. Returns the
/// residue ledger: reflective sends left in source shape, with the
/// reason.
pub fn apply_send_static_dispatch(
    app: &mut App,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Vec<Diagnostic> {
    let providers = collect_hash_providers(app);
    let defined = defined_method_name_counts(app);
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |body| {
        let elems = collect_var_element_sets(body);
        let origins = collect_provider_var_origins(body, &providers, &defined);
        rewrite(body, &elems, &providers, &origins, &[], registry, &mut diags);
    });
    diags
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    crate::lower::residue_diagnostic(
        "send_static_dispatch",
        "dynamic-send",
        expr.span,
        reason,
        format!(
            "reflective `send` left as dynamic dispatch ({reason}) — \
             strict targets cannot compile a runtime method name"
        ),
    )
}

// ---------------------------------------------------------------------
// Shape A/B: block param over a literal collection
// ---------------------------------------------------------------------

/// Per-local-variable literal element sets for one method body: the
/// variable was initialized to an array literal and only ever grown by
/// `push`/`<<`. Any other use of the variable (reassignment from a
/// non-literal, passing it as an argument, an un-allowlisted method
/// call) poisons the entry — the set can no longer be proven complete.
fn collect_var_element_sets(body: &Expr) -> HashMap<Symbol, Vec<Expr>> {
    let mut sets: HashMap<Symbol, Vec<Expr>> = HashMap::new();
    let mut poisoned: BTreeSet<Symbol> = BTreeSet::new();
    walk_var_uses(body, &mut sets, &mut poisoned);
    for name in poisoned {
        sets.remove(&name);
    }
    sets
}

fn walk_var_uses(
    e: &Expr,
    sets: &mut HashMap<Symbol, Vec<Expr>>,
    poisoned: &mut BTreeSet<Symbol>,
) {
    match &*e.node {
        ExprNode::Assign { target: crate::expr::LValue::Var { name, .. }, value } => {
            match &*value.node {
                ExprNode::Array { elements, .. } => {
                    // A reassignment replaces the set (last literal
                    // wins is not provable in general — poison on
                    // redefinition instead).
                    if sets.insert(name.clone(), elements.clone()).is_some() {
                        poisoned.insert(name.clone());
                    }
                }
                _ => {
                    poisoned.insert(name.clone());
                }
            }
            walk_var_uses(value, sets, poisoned);
        }
        ExprNode::Send { recv: Some(r), method, args, block, .. } => {
            if let ExprNode::Var { name, .. } = &*r.node {
                match method.as_str() {
                    // Growth: every arg joins the element set.
                    "push" | "<<" => {
                        if let Some(set) = sets.get_mut(name) {
                            set.extend(args.iter().cloned());
                        } else {
                            poisoned.insert(name.clone());
                        }
                    }
                    // Non-mutating reads the rewrite understands.
                    "each" | "map" | "flat_map" | "each_with_index" => {}
                    _ => {
                        poisoned.insert(name.clone());
                    }
                }
            } else {
                walk_var_uses(r, sets, poisoned);
            }
            for a in args {
                poison_bare_var(a, poisoned);
                walk_var_uses(a, sets, poisoned);
            }
            if let Some(b) = block {
                walk_var_uses(b, sets, poisoned);
            }
        }
        _ => {
            // A tracked var escaping into any other context (argument,
            // return value, interpolation) is handled conservatively by
            // the Send arm above for calls; other bare-var reads don't
            // let the collection mutate, so they're safe to ignore.
            e.node.for_each_child(&mut |c| walk_var_uses(c, sets, poisoned));
        }
    }
}

fn poison_bare_var(e: &Expr, poisoned: &mut BTreeSet<Symbol>) {
    if let ExprNode::Var { name, .. } = &*e.node {
        poisoned.insert(name.clone());
    }
}

/// One in-scope iteration binding: block param `name` ranges over the
/// literal elements `elems`.
struct IterBinding<'a> {
    name: &'a Symbol,
    elems: &'a [Expr],
}

fn rewrite(
    e: &mut Expr,
    var_sets: &HashMap<Symbol, Vec<Expr>>,
    providers: &HashProviders,
    origins: &HashMap<Symbol, (Symbol, Symbol)>,
    bindings: &[IterBinding<'_>],
    registry: &HashMap<ClassId, ClassInfo>,
    diags: &mut Vec<Diagnostic>,
) {
    // Iterator with a provable element set: descend into the block with
    // the binding pushed. Everything else recurses plainly; an inner
    // Lambda that rebinds one of our names shadows it.
    if let ExprNode::Send { recv: Some(r), method, args, block: Some(block), .. } = &mut *e.node {
        if matches!(method.as_str(), "each" | "map" | "flat_map")
            && args.is_empty()
        {
            let elems: Option<Vec<Expr>> = match &*r.node {
                ExprNode::Array { elements, .. } => Some(elements.clone()),
                ExprNode::Var { name, .. } => var_sets.get(name).cloned(),
                _ => None,
            };
            if let (Some(elems), ExprNode::Lambda { params, body, .. }) =
                (elems, &mut *block.node)
            {
                if params.len() == 1 {
                    let param = params[0].clone();
                    let mut inner: Vec<IterBinding<'_>> = Vec::new();
                    for b in bindings {
                        if *b.name != param {
                            inner.push(IterBinding { name: b.name, elems: b.elems });
                        }
                    }
                    inner.push(IterBinding { name: &param, elems: &elems });
                    rewrite(body, var_sets, providers, origins, &inner, registry, diags);
                    rewrite(r, var_sets, providers, origins, bindings, registry, diags);
                    return;
                }
            }
        }
    }
    if let ExprNode::Lambda { params, body, .. } = &mut *e.node {
        let survivors: Vec<IterBinding<'_>> = bindings
            .iter()
            .filter(|b| !params.contains(b.name))
            .map(|b| IterBinding { name: b.name, elems: b.elems })
            .collect();
        rewrite(body, var_sets, providers, origins, &survivors, registry, diags);
        return;
    }

    e.node.for_each_child_mut(&mut |c| {
        rewrite(c, var_sets, providers, origins, bindings, registry, diags)
    });

    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else {
        return;
    };
    if !is_send_method(method.as_str()) || args.is_empty() {
        return;
    }
    let (target, rest) = args.split_first().expect("non-empty checked above");
    // Literal name (`send(:title)`) is analyze Tier 1 — already a
    // compile-time-known dispatch, nothing to ground.
    if matches!(
        &*target.node,
        ExprNode::Lit { value: Literal::Sym { .. } | Literal::Str { .. } }
    ) {
        return;
    }
    if !recv_is_duplicable(recv) {
        diags.push(residue(e, "receiver is not an effect-free reader"));
        return;
    }
    let dispatch = enumerate_names(target, bindings, providers, origins);
    let Some(dispatch) = dispatch else {
        diags.push(residue(e, "method-name set not statically enumerable"));
        return;
    };
    if dispatch.names.is_empty() {
        // Provably no symbol can reach this send (the guard idioms make
        // such sites unreachable); leave the source shape alone rather
        // than emit a raise-only case.
        return;
    }
    let span = e.span;
    let arms = build_arms(&dispatch, recv, rest, span, registry);
    *e.node = ExprNode::Case { scrutinee: target.clone(), arms };
    // Keep the site type: analyze bounded the dynamic send by the
    // union of reachable method returns, and the case evaluates to
    // exactly one of them.
}

/// The enumerated dispatch: method names in first-seen order, plus
/// whether the scrutinee is a String (shape C's `.downcase`) or a
/// Symbol (the literal-collection shapes) — the `when` patterns must
/// match the scrutinee's class.
struct Dispatch {
    names: Vec<String>,
    string_scrutinee: bool,
}

fn push_unique(names: &mut Vec<String>, n: String) {
    if !names.iter().any(|x| x == &n) {
        names.push(n);
    }
}

fn enumerate_names(
    target: &Expr,
    bindings: &[IterBinding<'_>],
    providers: &HashProviders,
    origins: &HashMap<Symbol, (Symbol, Symbol)>,
) -> Option<Dispatch> {
    // Shape A/B: the arg is a block param (or a projection of one)
    // ranging over a literal collection.
    match &*target.node {
        // send(k) — k iterates the collection directly: every Symbol
        // element contributes; every other element must be a provably
        // non-Symbol literal or the set is unprovable.
        ExprNode::Var { name, .. } => {
            let b = bindings.iter().find(|b| b.name == name)?;
            let mut names = Vec::new();
            for el in b.elems {
                match &*el.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => {
                        push_unique(&mut names, value.as_str().to_string());
                    }
                    ExprNode::Lit { .. }
                    | ExprNode::Hash { .. }
                    | ExprNode::Array { .. }
                    | ExprNode::StringInterp { .. } => {}
                    _ => return None,
                }
            }
            Some(Dispatch { names, string_scrutinee: false })
        }
        // send(k.values.first) / send(k.keys.first) — k iterates the
        // collection; the name comes from each single-entry hash
        // literal's first value/key.
        ExprNode::Send { recv: Some(inner), method, args, .. }
            if method.as_str() == "first" && args.is_empty() =>
        {
            let ExprNode::Send { recv: Some(v), method: proj, args: pargs, .. } = &*inner.node
            else {
                return None;
            };
            if !pargs.is_empty() {
                return None;
            }
            let want_values = match proj.as_str() {
                "values" => true,
                "keys" => false,
                _ => return None,
            };
            let ExprNode::Var { name, .. } = &*v.node else { return None };
            let b = bindings.iter().find(|b| b.name == name)?;
            let mut names = Vec::new();
            for el in b.elems {
                match &*el.node {
                    ExprNode::Hash { entries, .. } => {
                        let (k, val) = entries.first()?;
                        let side = if want_values { val } else { k };
                        collect_symbol_outcomes(side, &mut names);
                    }
                    // Non-hash elements contribute nothing here (the
                    // `.values`/`.keys` projection is hash-guarded).
                    ExprNode::Lit { .. } | ExprNode::Array { .. } => {}
                    _ => return None,
                }
            }
            Some(Dispatch { names, string_scrutinee: false })
        }
        // Shape C: send(<hash-read>.downcase) — the string set comes
        // from a helper whose returns are hash literals.
        ExprNode::Send { recv: Some(inner), method, args, .. }
            if method.as_str() == "downcase" && args.is_empty() =>
        {
            let set = providers.string_set_of(inner, origins)?;
            let mut names = Vec::new();
            for s in set {
                push_unique(&mut names, s.to_lowercase());
            }
            Some(Dispatch { names, string_scrutinee: true })
        }
        _ => None,
    }
}

/// Every Symbol literal a hash-value expression can evaluate to, pushed
/// onto `names`. Conditionals contribute both branches; everything else
/// contributes nothing. This is deliberately lenient rather than
/// bailing: the `.values.first`/`.keys.first` projection idiom is the
/// `as_json` spec walk, whose sends sit behind an `is_a?(Symbol)`
/// guard, and whose non-literal hash values (`options[:with_comments]`,
/// `parent_comment && parent_comment.short_id`) are data — never
/// symbols — on every real path. If a run-time symbol ever does arrive
/// from one of them, the rewrite's wildcard arm raises loudly instead
/// of silently misdispatching.
fn collect_symbol_outcomes(e: &Expr, names: &mut Vec<String>) {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => {
            push_unique(names, value.as_str().to_string());
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            collect_symbol_outcomes(then_branch, names);
            collect_symbol_outcomes(else_branch, names);
        }
        _ => {}
    }
}

/// The rewrite duplicates the receiver into every arm, so it must be a
/// side-effect-free read. Implicit self / `self` always qualify; chains
/// of variable, ivar, constant, and `[]` reads do too.
fn recv_is_duplicable(recv: &Option<Expr>) -> bool {
    match recv {
        None => true,
        Some(e) => super::blank::is_effect_free_reader(e),
    }
}

/// ActiveSupport duration units in their singular form. When a
/// string-scrutinee dispatch's names are all duration units (lobsters'
/// `dur.send(intv.downcase).ago`), the arms call the plural form —
/// identical semantics on a numeric receiver, and the plural is what
/// the shared duration lowering (which runs right after this pass in
/// the hook) rewrites unconditionally into the Duration runtime.
fn duration_plural(name: &str) -> Option<&'static str> {
    Some(match name {
        "second" => "seconds",
        "minute" => "minutes",
        "hour" => "hours",
        "day" => "days",
        "week" => "weeks",
        "fortnight" => "fortnights",
        "month" => "months",
        "year" => "years",
        _ => return None,
    })
}

/// The return type dispatch would compute for `name` on the receiver's
/// class — own instance methods, then mixed-in modules, then the parent
/// chain, exactly the walk analyze's dispatch performs. `None` when the
/// receiver isn't a registered class or the method's return is still an
/// open inference variable.
fn method_return_via_registry(
    registry: &HashMap<ClassId, ClassInfo>,
    class: &ClassId,
    name: &str,
) -> Option<Ty> {
    let mut seen: BTreeSet<ClassId> = BTreeSet::new();
    let mut stack = vec![class.clone()];
    while let Some(cid) = stack.pop() {
        if !seen.insert(cid.clone()) {
            continue;
        }
        let Some(cls) = registry.get(&cid) else { continue };
        if let Some(ty) = cls.instance_methods.get(&Symbol::from(name)) {
            let ret = match ty {
                Ty::Fn { ret, .. } => (**ret).clone(),
                t => t.clone(),
            };
            return match ret {
                Ty::Var { .. } | Ty::Bottom => None,
                t => Some(t),
            };
        }
        stack.extend(cls.includes.iter().cloned());
        if let Some(p) = &cls.parent {
            stack.push(p.clone());
        }
    }
    None
}

/// The class the arms dispatch on, when the receiver types to one.
/// `None` receivers are implicit self — the hook walk carries no class
/// context, so those arms fall back to the gradual stamp.
fn recv_class(recv: &Option<Expr>) -> Option<ClassId> {
    let r = recv.as_ref()?;
    match r.ty.as_ref() {
        Some(Ty::Class { id, .. }) => Some(id.clone()),
        _ => None,
    }
}

fn build_arms(
    dispatch: &Dispatch,
    recv: &Option<Expr>,
    rest: &[Expr],
    span: Span,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Vec<Arm> {
    let all_durations = dispatch.string_scrutinee
        && dispatch.names.iter().all(|n| duration_plural(n).is_some());
    let class = recv_class(recv);
    let mut arms: Vec<Arm> = dispatch
        .names
        .iter()
        .map(|name| {
            let pattern = if dispatch.string_scrutinee {
                Pattern::Lit { value: Literal::Str { value: name.clone() } }
            } else {
                Pattern::Lit { value: Literal::Sym { value: Symbol::from(name.as_str()) } }
            };
            let called: Symbol = if all_durations {
                Symbol::from(duration_plural(name).expect("all_durations checked"))
            } else {
                Symbol::from(name.as_str())
            };
            let mut body = Expr::new(
                span,
                ExprNode::Send {
                    recv: recv.clone(),
                    method: called.clone(),
                    args: rest.to_vec(),
                    block: None,
                    parenthesized: !rest.is_empty(),
                },
            );
            // The residual-diagnostics audit walks hook output — an
            // unstamped send on a typed receiver reads as a dispatch
            // failure. Stamp what dispatch would compute; where the
            // registry can't answer (untyped receivers, duration-unit
            // arms awaiting the duration lowering), the honest type of
            // a formerly-reflective call is the gradual one.
            body.ty = Some(
                class
                    .as_ref()
                    .and_then(|c| method_return_via_registry(registry, c, called.as_str()))
                    .unwrap_or(Ty::Untyped),
            );
            Arm { pattern, guard: None, body }
        })
        .collect();
    // `send` raises NoMethodError on an unknown name; the wildcard arm
    // preserves that failure mode instead of silently returning nil.
    arms.push(Arm {
        pattern: Pattern::Wildcard,
        guard: None,
        body: Expr::new(
            span,
            ExprNode::Raise {
                value: Expr::new(
                    span,
                    ExprNode::Lit {
                        value: Literal::Str {
                            value: "dynamic send: method not in the statically \
                                    enumerated set"
                                .to_string(),
                        },
                    },
                ),
            },
        ),
    });
    arms
}

// ---------------------------------------------------------------------
// Shape C: string sets through hash-returning helpers
// ---------------------------------------------------------------------

/// For every class method whose *every* return expression is a hash
/// literal: the per-key sets of string values those literals can carry.
/// `providers.by_class_method[(class, method)][key]` is the full set of
/// strings the key can hold, present only when every return provably
/// pins it.
struct HashProviders {
    by_class_method: HashMap<(Symbol, Symbol), HashMap<Symbol, BTreeSet<String>>>,
    /// Provider keyed by bare method name, for attributing unqualified
    /// calls (`time_interval(...)` reaching a mixed-in helper). `None`
    /// marks a name multiple providers define — ambiguous, never
    /// attributed.
    by_method_name: HashMap<Symbol, Option<(Symbol, Symbol)>>,
}

impl HashProviders {
    /// The string set of `v[:key]` where `v` is a local assigned from a
    /// provider call (`length = time_interval(...)`; `length[:intv]`).
    fn string_set_of(
        &self,
        e: &Expr,
        origins: &HashMap<Symbol, (Symbol, Symbol)>,
    ) -> Option<&BTreeSet<String>> {
        let ExprNode::Send { recv: Some(v), method, args, .. } = &*e.node else {
            return None;
        };
        if method.as_str() != "[]" || args.len() != 1 {
            return None;
        }
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*args[0].node else {
            return None;
        };
        let ExprNode::Var { name, .. } = &*v.node else { return None };
        let origin = origins.get(name)?;
        self.by_class_method.get(origin)?.get(key)
    }
}

fn collect_hash_providers(app: &App) -> HashProviders {
    let mut by_class_method = HashMap::new();
    let mut register = |class: &str, consts: &HashMap<&str, &Expr>, name: &Symbol, body: &Expr| {
        if let Some(keysets) = hash_return_key_sets(body, consts) {
            by_class_method.insert((Symbol::from(class), name.clone()), keysets);
        }
    };
    for lc in &app.library_classes {
        // Constants visible to this class's method bodies.
        let consts: HashMap<&str, &Expr> = lc
            .constants
            .iter()
            .map(|(n, e)| (n.as_str(), e))
            .collect();
        for m in &lc.methods {
            register(lc.name.0.as_str(), &consts, &m.name, &m.body);
        }
    }
    for model in &app.models {
        let constants = super::model_to_library::collect_model_constants(model);
        let consts: HashMap<&str, &Expr> =
            constants.iter().map(|(n, e)| (n.as_str(), e)).collect();
        for item in &model.body {
            if let ModelBodyItem::Method { method, .. } = item {
                register(model.name.0.as_str(), &consts, &method.name, &method.body);
            }
        }
    }
    let mut by_method_name: HashMap<Symbol, Option<(Symbol, Symbol)>> = HashMap::new();
    for key in by_class_method.keys() {
        by_method_name
            .entry(key.1.clone())
            .and_modify(|e| *e = None)
            .or_insert_with(|| Some(key.clone()));
    }
    HashProviders { by_class_method, by_method_name }
}

/// Every user-defined method name in the app, with its definition
/// count. A bare provider call attributes only when the name resolves
/// uniquely app-wide — a second definition anywhere (even a
/// non-provider) means the call could dispatch elsewhere, so it never
/// grounds.
fn defined_method_name_counts(app: &App) -> HashMap<Symbol, usize> {
    let mut counts: HashMap<Symbol, usize> = HashMap::new();
    {
        let mut bump = |n: &Symbol| *counts.entry(n.clone()).or_insert(0) += 1;
        for model in &app.models {
            for item in &model.body {
                if let ModelBodyItem::Method { method, .. } = item {
                    bump(&method.name);
                }
            }
        }
        for lc in &app.library_classes {
            for m in &lc.methods {
                bump(&m.name);
            }
        }
        for c in &app.controllers {
            for item in &c.body {
                if let ControllerBodyItem::Action { action, .. } = item {
                    bump(&action.name);
                }
            }
        }
    }
    counts
}

/// Local vars in one body assigned from a provider call, matched in
/// source form: `Const.method(...)` (a single-segment constant
/// receiver) or a bare `method(...)` whose name is defined exactly
/// once app-wide (the mixed-in helper idiom — at this stage helper
/// calls are still unqualified). A reassignment from anything else
/// poisons the var.
fn collect_provider_var_origins(
    body: &Expr,
    providers: &HashProviders,
    defined: &HashMap<Symbol, usize>,
) -> HashMap<Symbol, (Symbol, Symbol)> {
    let mut origins: HashMap<Symbol, (Symbol, Symbol)> = HashMap::new();
    let mut poisoned: BTreeSet<Symbol> = BTreeSet::new();
    walk_provider_origins(body, providers, defined, &mut origins, &mut poisoned);
    for name in poisoned {
        origins.remove(&name);
    }
    origins
}

fn walk_provider_origins(
    e: &Expr,
    providers: &HashProviders,
    defined: &HashMap<Symbol, usize>,
    origins: &mut HashMap<Symbol, (Symbol, Symbol)>,
    poisoned: &mut BTreeSet<Symbol>,
) {
    if let ExprNode::Assign { target: crate::expr::LValue::Var { name, .. }, value } = &*e.node {
        let mut origin: Option<(Symbol, Symbol)> = None;
        if let ExprNode::Send { recv, method, .. } = &*value.node {
            match recv {
                Some(r) => {
                    if let ExprNode::Const { path } = &*r.node {
                        if path.len() == 1 {
                            let key = (path[0].clone(), method.clone());
                            if providers.by_class_method.contains_key(&key) {
                                origin = Some(key);
                            }
                        }
                    }
                }
                None => {
                    if defined.get(method).copied() == Some(1) {
                        if let Some(Some(key)) = providers.by_method_name.get(method) {
                            origin = Some(key.clone());
                        }
                    }
                }
            }
        }
        match origin {
            Some(o) => match origins.get(name) {
                Some(prev) if *prev != o => {
                    poisoned.insert(name.clone());
                }
                _ => {
                    origins.insert(name.clone(), o);
                }
            },
            // Reassigned from something else — no longer provably the
            // provider's hash.
            None => {
                poisoned.insert(name.clone());
            }
        }
    }
    e.node
        .for_each_child(&mut |c| walk_provider_origins(c, providers, defined, origins, poisoned));
}

/// If every return expression of `body` is a hash literal, the per-key
/// string sets across all of them; `None` otherwise. Only keys whose
/// value is provably a string set in *every* returned literal survive.
fn hash_return_key_sets(
    body: &Expr,
    consts: &HashMap<&str, &Expr>,
) -> Option<HashMap<Symbol, BTreeSet<String>>> {
    let mut literals: Vec<&Expr> = Vec::new();
    collect_return_positions(body, &mut literals)?;
    if literals.is_empty() {
        return None;
    }
    // Early returns anywhere in the body also produce values.
    if !collect_early_returns(body, &mut literals) {
        return None;
    }
    let mut sets: Option<HashMap<Symbol, BTreeSet<String>>> = None;
    for lit in &literals {
        let ExprNode::Hash { entries, .. } = &*lit.node else { return None };
        let mut this: HashMap<Symbol, BTreeSet<String>> = HashMap::new();
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                continue;
            };
            if let Some(strs) = string_values_of(v, consts) {
                this.insert(key.clone(), strs);
            }
        }
        sets = Some(match sets {
            None => this,
            Some(mut acc) => {
                // A key must be provable in every literal; union the
                // sets, drop keys missing from either side.
                acc.retain(|k, _| this.contains_key(k));
                for (k, s) in this {
                    if let Some(dst) = acc.get_mut(&k) {
                        dst.extend(s);
                    }
                }
                acc
            }
        });
    }
    sets
}

/// Tail-position expressions of a method body. `Some(())` when every
/// tail is a hash literal pushed into `out`; `None` on any other tail.
fn collect_return_positions<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) -> Option<()> {
    match &*e.node {
        ExprNode::Hash { .. } => {
            out.push(e);
            Some(())
        }
        ExprNode::Seq { exprs } => collect_return_positions(exprs.last()?, out),
        ExprNode::If { then_branch, else_branch, .. } => {
            collect_return_positions(then_branch, out)?;
            collect_return_positions(else_branch, out)
        }
        ExprNode::Case { arms, .. } => {
            for a in arms {
                collect_return_positions(&a.body, out)?;
            }
            Some(())
        }
        ExprNode::Return { value } => collect_return_positions(value, out),
        _ => None,
    }
}

/// Push every early-`Return` value; false when one isn't a hash literal.
fn collect_early_returns<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) -> bool {
    let mut ok = true;
    e.node.for_each_child(&mut |c| {
        if let ExprNode::Return { value } = &*c.node {
            if matches!(&*value.node, ExprNode::Hash { .. }) {
                out.push(value);
            } else {
                ok = false;
            }
        }
        if !collect_early_returns(c, out) {
            ok = false;
        }
    });
    ok
}

/// Every string a hash-value expression can evaluate to: a string
/// literal is itself; `CONST[x]` where CONST is a (frozen) hash literal
/// of string values is all of that hash's values.
fn string_values_of(e: &Expr, consts: &HashMap<&str, &Expr>) -> Option<BTreeSet<String>> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => {
            Some(std::iter::once(value.clone()).collect())
        }
        ExprNode::Send { recv: Some(r), method, .. } if method.as_str() == "[]" => {
            let ExprNode::Const { path } = &*r.node else { return None };
            if path.len() != 1 {
                return None;
            }
            let cval = consts.get(path[0].as_str())?;
            let hash = unwrap_freeze(cval);
            let ExprNode::Hash { entries, .. } = &*hash.node else { return None };
            let mut out = BTreeSet::new();
            for (_, v) in entries {
                let ExprNode::Lit { value: Literal::Str { value } } = &*v.node else {
                    return None;
                };
                out.insert(value.clone());
            }
            Some(out)
        }
        _ => None,
    }
}

fn unwrap_freeze(e: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*e.node {
        if method.as_str() == "freeze" && args.is_empty() {
            return r;
        }
    }
    e
}
