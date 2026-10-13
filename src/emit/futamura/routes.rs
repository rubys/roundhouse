//! Whole-route replacement: which routes the lowered app may serve.
//!
//! `--target futamura` ships the app's `--target ruby` emit beside the
//! app and lets it serve whole requests from a `Ruby::Box` inside the
//! Rails process (`bridge.rb`). A route goes to it only when nothing on
//! the route is left for Rails: an error diagnostic under the strict
//! `ruby` analysis means "this construct may be emitted wrong", so a
//! route whose code can reach one stays with Rails. That is the residue
//! policy of this target applied at route granularity, and it makes the
//! `ruby` target's error ledger read as a list of routes: clearing an
//! error moves the routes it blocked onto the lowered path.
//!
//! Reachability is computed over the classes the `ruby` target emits
//! (controllers with their filter chain spelled out in `process_action`,
//! views as module functions, models, library classes), starting from
//! one action's arm of `process_action`. It over-approximates: a send
//! whose receiver the analysis did not type reaches every app method of
//! that name, and a symbol literal naming an app method counts as a
//! call (callbacks, `send`). An error inside a reached method's source
//! range taints the route; an error outside every method (a class-level
//! `include`) taints routes that reach any method in its file.
//!
//! Also left to Rails, whatever the reachability says:
//!   * verbs other than GET (writes keep Rails' callbacks, including
//!     the ones gems add, which the lowered models do not have);
//!   * actions with a route that a route the lowered router refused
//!     could shadow (Rails would match the refused one first).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::App;
use crate::diagnostic::{Diagnostic, Severity};
use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal, Pattern};
use crate::ident::Symbol;
use crate::lower::routes::FlatRoute;
use crate::ty::Ty;

/// What the plan decided, for the initializer and the CLI's report.
#[derive(Default, Debug)]
pub struct RoutePlan {
    /// `controller#action`, keyed as the lowered router names them
    /// (`home#index`).
    pub actions: Vec<String>,
    /// One line per GET action left to Rails, with the reason.
    pub residue: Vec<String>,
    /// Errors inside no method body (a class-level `include`); they
    /// block every route that reaches a method in their file.
    pub unowned: Vec<String>,
}

type MethodKey = (String, Symbol, bool); // (class, method, class-side)

struct Index<'a> {
    /// Every class of a name: each view template is its own
    /// `Views::Home`, so one name can be many classes.
    classes: HashMap<String, Vec<&'a LibraryClass>>,
    /// method name -> every (class, side) defining it, for fan-out.
    by_name: HashMap<Symbol, Vec<(String, bool)>>,
}

impl<'a> Index<'a> {
    fn new(lcs: &'a [LibraryClass]) -> Self {
        let mut classes: HashMap<String, Vec<&'a LibraryClass>> = HashMap::new();
        let mut by_name: HashMap<Symbol, Vec<(String, bool)>> = HashMap::new();
        for lc in lcs {
            classes.entry(lc.name.0.as_str().to_string()).or_default().push(lc);
            for m in &lc.methods {
                by_name
                    .entry(m.name.clone())
                    .or_default()
                    .push((lc.name.0.as_str().to_string(), is_class_side(&m.receiver)));
            }
        }
        Index { classes, by_name }
    }

    /// Every body of `class`'s `name` on one side (a reopened class or a
    /// module spread over files can define it more than once; walking
    /// each over-approximates, which is the safe direction).
    fn bodies(&self, class: &str, name: &Symbol, class_side: bool) -> Vec<&'a crate::dialect::MethodDef> {
        self.classes
            .get(class)
            .into_iter()
            .flatten()
            .flat_map(|lc| lc.methods.iter())
            .filter(|m| &m.name == name && is_class_side(&m.receiver) == class_side)
            .collect()
    }

    /// The class (walking parents and includes) that defines `name` on
    /// the given side, as Ruby's lookup would find it.
    fn lookup(&self, class: &str, name: &Symbol, class_side: bool) -> Option<MethodKey> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([class.to_string()]);
        while let Some(c) = queue.pop_front() {
            if !seen.insert(c.clone()) {
                continue;
            }
            let Some(lcs) = self.classes.get(&c) else { continue };
            if !self.bodies(&c, name, class_side).is_empty() {
                return Some((c, name.clone(), class_side));
            }
            for lc in lcs {
                for inc in &lc.includes {
                    queue.push_back(inc.0.as_str().to_string());
                }
                if let Some(p) = &lc.parent {
                    queue.push_back(p.0.as_str().to_string());
                }
            }
        }
        None
    }
}

fn is_class_side(r: &MethodReceiver) -> bool {
    !matches!(r, MethodReceiver::Instance)
}

/// Decide which GET actions the lowered app serves.
///
/// `lowered` is the app after `analyze_and_lower` (what the `ruby`
/// target emits from); `diags` every diagnostic that emit reported.
pub fn plan(lowered: &App, diags: &[Diagnostic]) -> RoutePlan {
    let mut lcs = crate::emit::ruby::lowered_controller_lcs_with_layout(lowered);
    lcs.extend(crate::emit::ruby::lowered_view_lcs(lowered).1);
    lcs.extend(crate::emit::ruby::lowered_model_lcs(lowered).0);
    lcs.extend(lowered.library_classes.iter().cloned());
    let index = Index::new(&lcs);
    // Controllers: their actions keep the app's spans under a
    // re-synthesized header.
    let controllers: HashSet<String> = lowered.controllers.iter().map(|c| c.name.0.as_str().to_string()).collect();
    let ranges = method_ranges(&controllers, &lcs);
    let exact = expression_owners(&controllers, &lcs);
    let owners = |d: &Diagnostic| -> Vec<&MethodKey> {
        match exact.get(&(d.span.file.0, d.span.start)) {
            Some(ms) => ms.iter().collect(),
            None => containing_methods(&ranges, d),
        }
    };

    let errors: Vec<&Diagnostic> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    let routes_file = lowered
        .sources
        .iter()
        .position(|s| s.path.ends_with("config/routes.rb"))
        .map(|i| i as u32 + 1);

    let location = |d: &Diagnostic| -> String {
        if d.span.file.0 == 0 {
            return String::from("(no location)");
        }
        let source = &lowered.sources[d.span.file.0 as usize - 1];
        let (line, _) = source.line_col(d.span.start);
        let rel = source.path.rsplit_once("/app/").map(|(_, r)| format!("app/{r}")).unwrap_or_else(|| source.path.clone());
        format!("{rel}:{line}")
    };

    // Every flattened route in declaration order, refused ones included:
    // a refused route earlier in the table is one Rails matches and the
    // lowered router does not.
    let all = crate::lower::routes::flatten_routes_for_helpers(lowered);
    let mut by_action: BTreeMap<String, Vec<(usize, &FlatRoute)>> = BTreeMap::new();
    for (i, r) in all.iter().enumerate() {
        if r.refused.is_some() || r.method != crate::dialect::HttpMethod::Get {
            continue;
        }
        let key = format!(
            "{}#{}",
            crate::lower::routes_to_library::controller_symbol(r.controller.0.as_str()),
            r.action.as_str()
        );
        by_action.entry(key).or_default().push((i, r));
    }

    let mut out = RoutePlan::default();
    // `ROUNDHOUSE_FUTAMURA_LEDGER=2`: each error and the methods owning it.
    if std::env::var("ROUNDHOUSE_FUTAMURA_LEDGER").as_deref() == Ok("2") {
        for d in &errors {
            let names: Vec<String> = owners(d).iter().map(|(c, n, side)| format!("{c}{}{}", if *side { "." } else { "#" }, n.as_str())).collect();
            eprintln!("  owner       {} -> {}", location(d), names.join(", "));
        }
    }
    // Errors no method owns fall back to file-level attribution, and an
    // error with no location at all could be anywhere, so it blocks every
    // route. List both, so the fallback is visible.
    for d in &errors {
        if Some(d.span.file.0) != routes_file && (d.span.file.0 == 0 || owners(d).is_empty()) {
            out.unowned.push(format!("{} {}", location(d), d.message.lines().next().unwrap_or("")));
        }
    }
    for (key, routes) in &by_action {
        let (_, first) = routes[0];
        let label = format!("GET {} {key}", first.path);
        // Shadowing by a refused route.
        if let Some(refused) = routes.iter().find_map(|(i, r)| {
            all[..*i].iter().find(|o| o.refused.is_some() && may_overlap(&o.path, &r.path))
        }) {
            out.residue.push(format!("{label}: {} {} is refused by the lowered router and matches first", format!("{:?}", refused.method).to_uppercase(), refused.path));
            continue;
        }
        let controller = first.controller.0.as_str();
        if !index.classes.contains_key(controller) {
            out.residue.push(format!("{label}: {controller} is not lowered"));
            continue;
        }
        let reached = reach(&index, controller, first.action.as_str());
        let mut reached_files: HashSet<u32> = HashSet::new();
        for m in reached.keys() {
            if let Some(rs) = ranges.get(m) {
                reached_files.extend(rs.keys().copied());
            }
        }
        let blocking: Vec<&Diagnostic> = errors
            .iter()
            .copied()
            .filter(|d| Some(d.span.file.0) != routes_file)
            .filter(|d| d.span.file.0 == 0 || match owners(d) {
                ms if ms.is_empty() => reached_files.contains(&d.span.file.0),
                ms => ms.iter().any(|m| reached.contains_key(*m)),
            })
            .collect();
        if blocking.is_empty() {
            out.actions.push(key.clone());
        } else {
            let first_err = blocking[0];
            // The call chain to the first error, so the ledger says how
            // the route gets there, not only that it does.
            let via = owners(first_err)
                .into_iter()
                .find(|m| reached.contains_key(*m))
                .map(|m| {
                    let mut chain = vec![m.clone()];
                    while let Some(Some(p)) = reached.get(chain.last().unwrap()) {
                        if chain.contains(p) || chain.len() > 32 {
                            break;
                        }
                        chain.push(p.clone());
                    }
                    chain.reverse();
                    let names: Vec<String> = chain
                        .iter()
                        .map(|(c, n, side)| format!("{c}{}{}", if *side { "." } else { "#" }, n.as_str()))
                        .collect();
                    format!(" via {}", names.join(" -> "))
                })
                .unwrap_or_default();
            out.residue.push(format!(
                "{label}: reaches {} error(s), first {} {}{via}",
                blocking.len(),
                location(first_err),
                first_err.message.lines().next().unwrap_or("")
            ));
        }
    }
    out
}

/// Can an error be attributed to this method's body? The lowering
/// synthesizes methods (attribute readers and writers, `[]`, `[]=`,
/// `from_stmt`, `initialize`, whole row classes like lobsters' `StoryRow`)
/// whose spans are borrowed from elsewhere in the source: the synthesized
/// `Story#short_id=` starts where the app's own `def url=` does, and
/// would claim that method's errors. So only a `def` the app wrote (a
/// real `name_span`) owns its body's spans, plus controller actions and
/// views, which are lowered from the app's code span for span though
/// their headers are re-synthesized.
fn owns_source(lowered_by_name: &HashSet<String>, lc: &LibraryClass, m: &crate::dialect::MethodDef) -> bool {
    m.kind == crate::dialect::AccessorKind::Method
        && (!m.name_span.is_synthetic()
            || lowered_by_name.contains(lc.name.0.as_str())
            || lc.name.0.as_str().starts_with("Views::"))
}

/// Per method, the source range its body covers in each file.
fn method_ranges(controllers: &HashSet<String>, lcs: &[LibraryClass]) -> HashMap<MethodKey, BTreeMap<u32, (u32, u32)>> {
    let mut out = HashMap::new();
    for lc in lcs {
        for m in lc.methods.iter().filter(|m| owns_source(controllers, lc, m)) {
            let mut ranges: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
            visit(&m.body, &mut |e| {
                let s = e.span;
                if s.file.0 == 0 || s.end <= s.start {
                    return;
                }
                let r = ranges.entry(s.file.0).or_insert((s.start, s.end));
                r.0 = r.0.min(s.start);
                r.1 = r.1.max(s.end);
            });
            if !ranges.is_empty() {
                out.entry((lc.name.0.as_str().to_string(), m.name.clone(), is_class_side(&m.receiver)))
                    .or_insert(ranges);
            }
        }
    }
    out
}

/// Per expression start, the methods whose bodies hold it: where an
/// error reported at that expression lives in the emitted program.
fn expression_owners(controllers: &HashSet<String>, lcs: &[LibraryClass]) -> HashMap<(u32, u32), Vec<MethodKey>> {
    let mut out: HashMap<(u32, u32), Vec<MethodKey>> = HashMap::new();
    for lc in lcs {
        for m in lc.methods.iter().filter(|m| owns_source(controllers, lc, m)) {
            let key = (lc.name.0.as_str().to_string(), m.name.clone(), is_class_side(&m.receiver));
            visit(&m.body, &mut |e| {
                if e.span.file.0 == 0 {
                    return;
                }
                let owners = out.entry((e.span.file.0, e.span.start)).or_default();
                if !owners.contains(&key) {
                    owners.push(key.clone());
                }
            });
        }
    }
    out
}

fn containing_methods<'m>(
    ranges: &'m HashMap<MethodKey, BTreeMap<u32, (u32, u32)>>,
    d: &Diagnostic,
) -> Vec<&'m MethodKey> {
    ranges
        .iter()
        .filter_map(|(k, rs)| {
            let (s, e) = rs.get(&d.span.file.0)?;
            (*s <= d.span.start && d.span.start < *e).then_some((k, e - s))
        })
        .fold(Vec::<(&MethodKey, u32)>::new(), |mut best, (k, width)| {
            // The innermost: a synthesized method (`process_action`, an
            // attribute reader) carries spans from across its class, and
            // would otherwise claim every error in the file.
            match best.first().map(|(_, w)| *w) {
                Some(w) if width > w => {}
                Some(w) if width < w => best = vec![(k, width)],
                _ => best.push((k, width)),
            }
            best
        })
        .into_iter()
        .map(|(k, _)| k)
        .collect()
}

/// Every app method one action can run: its arm of `process_action`
/// (the filters that apply to it, then the action), closed over calls.
fn reach(index: &Index, controller: &str, action: &str) -> HashMap<MethodKey, Option<MethodKey>> {
    let mut reached: HashMap<MethodKey, Option<MethodKey>> = HashMap::new();
    let mut queue: VecDeque<MethodKey> = VecDeque::new();
    let process = Symbol::from("process_action");
    match index.lookup(controller, &process, false) {
        Some(key) => {
            reached.insert(key.clone(), None);
            for m in index.bodies(&key.0, &process, false) {
                calls_for_action(&m.body, action, &mut |e| {
                    enqueue_send(index, e, controller, false, &key, &mut reached, &mut queue)
                });
            }
        }
        // No dispatcher: the action is the whole route.
        None => {
            if let Some(key) = index.lookup(controller, &Symbol::from(action), false) {
                reached.insert(key.clone(), None);
                queue.push_back(key);
            }
        }
    }
    while let Some(from) = queue.pop_front() {
        let (class, name, class_side) = &from;
        for m in index.bodies(class, name, *class_side) {
            visit(&m.body, &mut |e| enqueue_send(index, e, class, *class_side, &from, &mut reached, &mut queue));
        }
    }
    reached
}

/// Walk `process_action`'s body for one action: `if [..].include?(
/// action_name)` and `case action_name` are decided, everything else is
/// walked whole.
fn calls_for_action(e: &Expr, action: &str, f: &mut dyn FnMut(&Expr)) {
    match &*e.node {
        ExprNode::If { cond, then_branch, else_branch } => match action_test(cond, action) {
            Some(true) => calls_for_action(then_branch, action, f),
            Some(false) => calls_for_action(else_branch, action, f),
            None => {
                calls_for_action(cond, action, f);
                calls_for_action(then_branch, action, f);
                calls_for_action(else_branch, action, f);
            }
        },
        ExprNode::Case { scrutinee, arms } if is_action_name(scrutinee) => {
            let hit = arms.iter().find(|a| match &a.pattern {
                Pattern::Lit { value: Literal::Sym { value } } => value.as_str() == action,
                _ => false,
            });
            match hit {
                Some(arm) => calls_for_action(&arm.body, action, f),
                // No arm: every unmatched arm (an `else`) may run.
                None => {
                    for a in arms.iter().filter(|a| !matches!(a.pattern, Pattern::Lit { .. })) {
                        calls_for_action(&a.body, action, f);
                    }
                }
            }
        }
        _ => {
            f(e);
            e.node.for_each_child(&mut |c| calls_for_action(c, action, f));
        }
    }
}

fn is_action_name(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Var { name, .. } if name.as_str() == "action_name")
}

/// `[:a, :b].include?(action_name)`, possibly negated.
fn action_test(cond: &Expr, action: &str) -> Option<bool> {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*cond.node else { return None };
    match method.as_str() {
        "!" if args.is_empty() => action_test(recv, action).map(|b| !b),
        "include?" if args.len() == 1 && is_action_name(&args[0]) => {
            let ExprNode::Array { elements, .. } = &*recv.node else { return None };
            let mut found = false;
            for el in elements {
                match &*el.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => found |= value.as_str() == action,
                    _ => return None,
                }
            }
            Some(found)
        }
        _ => None,
    }
}

fn enqueue_send(
    index: &Index,
    e: &Expr,
    class: &str,
    class_side: bool,
    from: &MethodKey,
    reached: &mut HashMap<MethodKey, Option<MethodKey>>,
    queue: &mut VecDeque<MethodKey>,
) {
    let mut push = |k: MethodKey| {
        if !reached.contains_key(&k) {
            reached.insert(k.clone(), Some(from.clone()));
            queue.push_back(k);
        }
    };
    // A value the analysis did not type is an instance: a class object
    // reaches here as a constant, which is resolved exactly.
    let fan_out = |name: &Symbol, push: &mut dyn FnMut(MethodKey)| {
        for (c, side) in index.by_name.get(name).into_iter().flatten() {
            if !*side {
                push((c.clone(), name.clone(), false));
            }
        }
    };
    match &*e.node {
        ExprNode::Send { recv, method, .. } => match recv.as_ref().map(|r| (&*r.node, r.ty.as_ref())) {
            None | Some((ExprNode::SelfRef, _)) => {
                match index.lookup(class, method, class_side) {
                    Some(k) => push(k),
                    // Not on this class: a runtime method (`render`,
                    // `params`), or a helper module's function the
                    // lowering left receiverless; any app definer of
                    // either side may run.
                    None => {
                        for (c, side) in index.by_name.get(method).into_iter().flatten() {
                            push((c.clone(), method.clone(), *side));
                        }
                    }
                }
            }
            Some((ExprNode::Const { path }, _)) => {
                let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                if let Some(k) = index.lookup(&name, method, true) {
                    push(k);
                } else if method.as_str() == "new" {
                    if let Some(k) = index.lookup(&name, &Symbol::from("initialize"), false) {
                        push(k);
                    }
                }
            }
            // `Rails.cache.fetch`, `ActiveRecord::Base.connection.x`: a
            // framework object, whose methods are the runtime's.
            Some(_) if rooted_in_framework(index, recv.as_ref().unwrap()) => {}
            Some((_, ty)) => {
                let classes = ty.map(classes_of).unwrap_or_default();
                if classes.is_empty() {
                    fan_out(method, &mut push);
                } else {
                    for c in classes {
                        for side in [false, true] {
                            if let Some(k) = index.lookup(&c, method, side) {
                                push(k);
                            }
                        }
                    }
                }
            }
        },
        ExprNode::Super { .. } => {
            // The parent's definition of the method being walked runs;
            // which one is not recorded on the node, so every definer.
            for lc in index.classes.get(class).into_iter().flatten() {
                if let Some(p) = &lc.parent {
                    for m in &lc.methods {
                        if let Some(k) = index.lookup(p.0.as_str(), &m.name, is_class_side(&m.receiver)) {
                            push(k);
                        }
                    }
                }
            }
        }
        ExprNode::MethodRef { name, .. } => fan_out(name, &mut push),
        // `send(:x)`, `public_send(:x)`, a callback named by symbol: a
        // method of the class whose code holds the symbol.
        ExprNode::Lit { value: Literal::Sym { value } } => {
            for side in [false, true] {
                if let Some(k) = index.lookup(class, value, side) {
                    push(k);
                }
            }
        }
        _ => {}
    }
}

/// Is `e` a chain of sends rooted in a constant the app does not define?
fn rooted_in_framework(index: &Index, e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Const { path } => {
            let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            !index.classes.contains_key(&name)
        }
        ExprNode::Send { recv: Some(r), .. } => rooted_in_framework(index, r),
        // `request.env`, `params[:x]`, `cookies`: a receiverless call no
        // app class answers is the framework's accessor.
        ExprNode::Send { recv: None, method, .. } => !index.by_name.contains_key(method),
        _ => false,
    }
}

/// App classes a typed receiver can be. Empty means "unknown": fan out.
fn classes_of(ty: &Ty) -> Vec<String> {
    match ty {
        Ty::Class { id, .. } => vec![id.0.as_str().to_string()],
        Ty::Union { variants } => {
            let mut out = Vec::new();
            for v in variants {
                match v {
                    Ty::Nil => {}
                    Ty::Class { .. } | Ty::Union { .. } => out.extend(classes_of(v)),
                    // A primitive variant answers no app method; only
                    // the class variants can reach app code.
                    _ => {}
                }
            }
            out
        }
        // Primitives, containers and the like: runtime methods only.
        Ty::Untyped | Ty::Var { .. } => Vec::new(),
        _ => vec![String::from("\0none")],
    }
}

fn visit(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    e.node.for_each_child(&mut |c| visit(c, f));
}

/// Could one URL match both patterns? Same segment count, and every
/// pair of literal segments equal; a dynamic or glob segment matches
/// anything. Conservative: "maybe" is yes.
fn may_overlap(a: &str, b: &str) -> bool {
    let seg = |p: &str| -> Vec<String> {
        p.trim_end_matches("(.:format)").split('/').filter(|s| !s.is_empty()).map(String::from).collect()
    };
    let (sa, sb) = (seg(a), seg(b));
    if sa.iter().chain(sb.iter()).any(|s| s.starts_with('*') || s.contains('(')) {
        return true;
    }
    if sa.len() != sb.len() {
        return false;
    }
    let dynamic = |s: &str| s.starts_with(':') || s.contains(':');
    sa.iter().zip(sb.iter()).all(|(x, y)| dynamic(x) || dynamic(y) || x == y)
}

#[cfg(test)]
mod tests {
    use super::may_overlap;

    #[test]
    fn overlap_is_segmentwise() {
        assert!(may_overlap("/domains/:id", "/domains/:name"));
        assert!(may_overlap("/domains/:id", "/domains/new"));
        assert!(!may_overlap("/domains/:id", "/domain/:id"));
        assert!(!may_overlap("/domains/:id/edit", "/domains/:id"));
        assert!(may_overlap("/files/*path", "/anything/at/all"));
    }
}
