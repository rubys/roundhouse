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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use crate::App;
use crate::diagnostic::{Diagnostic, Severity};
use crate::dialect::{ControllerBodyItem, LibraryClass, MethodReceiver};
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
    /// INTERIM (see `enqueue_send`'s fallback): the analyzer's own type
    /// for every expression in the app's controller bodies, from before
    /// `routes::plan`'s own re-lowering re-types them from scratch and
    /// loses framework/private-helper receiver types (G2/G3, tracked
    /// upstream as rubys/roundhouse#58). Keyed by exact source span
    /// (file, start, end), since the re-lowering reads the same spans
    /// without moving them.
    analyzer_stamps: HashMap<(u32, u32, u32), Ty>,
}

impl<'a> Index<'a> {
    fn new(lcs: &'a [LibraryClass], analyzer_stamps: HashMap<(u32, u32, u32), Ty>) -> Self {
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
        Index { classes, by_name, analyzer_stamps }
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

    /// Is `class` declared with `module` rather than `class`? A name can
    /// be reopened across files (one view template per file shares a
    /// name with its siblings); any declaration saying "module" is
    /// enough, since a mixin can only be a module in the first place.
    fn is_module(&self, class: &str) -> bool {
        self.classes.get(class).into_iter().flatten().any(|lc| lc.is_module)
    }
}

fn is_class_side(r: &MethodReceiver) -> bool {
    !matches!(r, MethodReceiver::Instance)
}

/// Trim whatever root a path was walked from down to its `app/...`
/// tail, the shape both a diagnostic's resolved location and an ingest
/// gap's file (the survey's `IngestError::Unsupported.file`, an
/// absolute path under the surveyed fixture) are compared in.
fn app_relative(path: &str) -> String {
    // `IngestError::Unsupported.file` is `Path::display()` of wherever
    // the survey walked, which on Windows carries `\` separators; the
    // match below (and `view_prefix`'s `/`-joined prefix) only ever
    // looks for `/`, so a backslash path would never trim down to
    // `app/...` and a gap on it would silently stop blocking its route.
    let path = path.replace('\\', "/");
    path.rsplit_once("/app/").map(|(_, r)| format!("app/{r}")).unwrap_or(path)
}

/// A span's file the way the ingest-gap ledger's paths are shaped:
/// `app/...`, trimmed of whatever root the survey ran from. `None` for
/// a synthesized span (no location at all).
fn rel_path(lowered: &App, file: u32) -> Option<String> {
    if file == 0 {
        return None;
    }
    Some(app_relative(&lowered.sources[file as usize - 1].path))
}

/// Rails' naming convention for the view template an action renders:
/// `app/views/<dir>/<action>.`, the format and handler extensions
/// following. Namespaces nest as directories
/// (`WellKnown::HostMetaController` -> `well_known/host_meta`), unlike
/// the flattened form `controller_symbol` builds for the route-helper
/// table and the ledger's own keys.
fn view_prefix(controller_class: &str, action: &str) -> String {
    let base = controller_class.strip_suffix("Controller").unwrap_or(controller_class);
    format!("app/views/{}/{action}.", crate::naming::underscore(base))
}

/// Does a recorded ingest gap mean this route cannot be trusted? A gap
/// has no span (survey mode substitutes a nil placeholder and keeps
/// going instead of failing ingest), so it never reaches `diags` and
/// the usual blocking filter never sees it — yet it still names a file,
/// which may be: the route's own controller (a class-body macro or
/// block the ingest never modeled: `serialization_scope`,
/// `content_security_policy`), a file in the controller's ancestor
/// chain (the same kind of gap in a parent controller — `Admin::Base
/// Controller`'s dropped CSP block applies to every subclass, though
/// the gap only names the parent's file), any file the route's reach
/// actually walks into (a partial or layout inlined into the view
/// method that renders it), or the view template the action renders by
/// convention (a template engine not ingested at all: `.css.erb`,
/// `.xml.ruby` — these never become a method, so reach can't see them,
/// which is why the prefix check stays). Either way the lowered app
/// cannot serve what the gap dropped, so the route it belongs to stays
/// with Rails.
fn gap_blocking<'g>(
    gaps: &'g [(String, String)],
    blocking_files: &HashSet<String>,
    prefix: &str,
) -> Option<&'g (String, String)> {
    gaps.iter().find(|(file, _)| {
        let rel = app_relative(file);
        if blocking_files.contains(&rel) || rel.starts_with(prefix) {
            return true;
        }
        // A gap in an `app/helpers/*_helper.rb` module is never a file
        // `reach` can walk into: Rails ingest only registers a helper
        // module's methods into `helper_method_index`, never into a
        // `LibraryClass` `reach` could add to `blocking_files` — the
        // file rejected ingest outright, so there is no method body to
        // find it through in the first place. Yet Rails mixes EVERY
        // helper module into EVERY view (`helper :all`, the default),
        // so whatever the gap dropped is missing from every view's
        // surface, not just one route's. Block any route that renders
        // a view at all.
        is_helper_gap(&rel) && blocking_files.iter().any(|f| f.starts_with("app/views/"))
    })
}

/// Rails' own naming convention for which `app/helpers/` files get
/// mixed into every view (`all_helpers_from_path` globs
/// `**/*_helper.rb`, nothing else — see `ingest/app.rs`'s
/// `is_helper_module`). Matched the same way here so a gap on, say, a
/// plain PORO that happens to live under `app/helpers/` does not
/// over-block routes it was never mixed into.
fn is_helper_gap(rel: &str) -> bool {
    rel.strip_prefix("app/helpers/")
        .and_then(|tail| tail.strip_suffix(".rb"))
        .is_some_and(|stem| {
            stem.rsplit('/').next().is_some_and(|name| name.ends_with("_helper"))
        })
}

/// Every class above `class` found by walking `parent` alone (never
/// `includes` — a mixed-in concern's gap is covered by `reached_files`
/// instead, through whichever reached method actually pulls it in):
/// `ApplicationController` is a `WidgetsController`'s ancestor this way.
fn ancestor_chain(index: &Index, class: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([class.to_string()]);
    let mut cur = class.to_string();
    while let Some(parent) = index.classes.get(&cur).into_iter().flatten().find_map(|lc| lc.parent.clone()) {
        let parent = parent.0.as_str().to_string();
        if !seen.insert(parent.clone()) {
            break;
        }
        out.push(parent.clone());
        cur = parent;
    }
    out
}

/// Decide which GET actions the lowered app serves.
///
/// `lowered` is the app after `analyze_and_lower` (what the `ruby`
/// target emits from); `diags` every diagnostic that emit reported;
/// `gaps` every recorded ingest gap, as `(file, message)` — these carry
/// no span and never become a diagnostic, so they are attributed here
/// instead of through the usual blocking filter below.
pub fn plan(lowered: &App, diags: &[Diagnostic], gaps: &[(String, String)]) -> RoutePlan {
    // Re-lowering pushes the `ruby` emit's refusals into whatever
    // diagnostic scope is active, which here is futamura's own. They are
    // already in `diags` (`project::futamura_whole_route`); collected
    // again they would fail an emit that reports nothing.
    let (mut lcs, _) = crate::emit::diagnostics::scope(|| {
        let mut lcs = crate::emit::ruby::lowered_controller_lcs_with_layout(lowered);
        lcs.extend(crate::emit::ruby::lowered_view_lcs(lowered).1);
        lcs.extend(crate::emit::ruby::lowered_model_lcs(lowered).0);
        lcs
    });
    lcs.extend(lowered.library_classes.iter().cloned());
    let index = Index::new(&lcs, analyzer_stamps(lowered));
    // Controllers: their actions keep the app's spans under a
    // re-synthesized header.
    let controllers: HashSet<String> = lowered.controllers.iter().map(|c| c.name.0.as_str().to_string()).collect();
    let ranges = method_ranges(&controllers, &lcs);
    // Every rel-path file a controller's own methods are written in —
    // almost always one file, but a controller can reopen across files
    // the way a reopened model does.
    let mut controller_files: HashMap<String, HashSet<String>> = HashMap::new();
    for ((class, _, _), rs) in &ranges {
        if !controllers.contains(class) {
            continue;
        }
        let files = controller_files.entry(class.clone()).or_default();
        files.extend(rs.keys().filter_map(|&f| rel_path(lowered, f)));
    }
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
        let rel = rel_path(lowered, d.span.file.0).unwrap_or_else(|| source.path.clone());
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
    let mut callees: HashMap<MethodKey, Vec<MethodKey>> = HashMap::new();
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
        let reached = reach(&index, &mut callees, controller, first.action.as_str());
        let mut reached_files: HashSet<u32> = HashSet::new();
        for m in reached.keys() {
            if let Some(rs) = ranges.get(m) {
                reached_files.extend(rs.keys().copied());
            }
        }
        // Every file whose ingest gap must block this route: the
        // controller's own file, any file in its ancestor chain (the
        // same files `reach`'s `Super` handling and `enqueue_send`'s
        // lookup walk), and every file the route's reach actually
        // touches — partials and layouts are inlined into the view
        // method that renders them, so a gap in one shows up here.
        let mut blocking_files: HashSet<String> = controller_files.get(controller).cloned().unwrap_or_default();
        for ancestor in ancestor_chain(&index, controller) {
            if let Some(fs) = controller_files.get(&ancestor) {
                blocking_files.extend(fs.iter().cloned());
            }
        }
        blocking_files.extend(reached_files.iter().filter_map(|&f| rel_path(lowered, f)));
        if let Some((file, message)) = gap_blocking(gaps, &blocking_files, &view_prefix(controller, first.action.as_str())) {
            out.residue.push(format!("{label}: ingest gap in {file}: {message}"));
            continue;
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
    let mut innermost: Vec<&MethodKey> = ranges
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
        .collect();
    // Sorted: the ledger's `via` follows the first owner a route
    // reaches, and the map's order changes from run to run.
    innermost.sort();
    innermost
}

/// Every app method one action can run: its arm of `process_action`
/// (the filters that apply to it, then the action), closed over calls.
fn reach(
    index: &Index,
    callees: &mut HashMap<MethodKey, Vec<MethodKey>>,
    controller: &str,
    action: &str,
) -> HashMap<MethodKey, Option<MethodKey>> {
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
        let next = callees.entry(from.clone()).or_insert_with(|| callees_of(index, &from));
        for k in next.iter() {
            if !reached.contains_key(k) {
                reached.insert(k.clone(), Some(from.clone()));
                queue.push_back(k.clone());
            }
        }
    }
    reached
}

/// What a method's body may call, in the order the walk meets it. It
/// does not depend on the route that reached the method, so `plan` keeps
/// one answer per method across routes: on Mastodon nearly every route
/// reaches nearly the whole app, and re-walking it per route was most of
/// the emit.
fn callees_of(index: &Index, key: &MethodKey) -> Vec<MethodKey> {
    let (class, name, class_side) = key;
    let mut seen: HashMap<MethodKey, Option<MethodKey>> = HashMap::new();
    let mut found: VecDeque<MethodKey> = VecDeque::new();
    for m in index.bodies(class, name, *class_side) {
        visit(&m.body, &mut |e| enqueue_send(index, e, class, *class_side, key, &mut seen, &mut found));
    }
    found.into()
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
                    // Not on this class (nor its ancestors or included
                    // modules, which `index.lookup` already walked): a
                    // runtime method (`render`, `params`), or a helper
                    // module's function the lowering left receiverless.
                    // A receiverless call dispatches on `self`, so it can
                    // never reach an unrelated concrete class that
                    // happens to define the same name — only a module,
                    // the shape a mixed-in helper takes.
                    None => {
                        for (c, side) in index.by_name.get(method).into_iter().flatten() {
                            if index.is_module(c) {
                                push((c.clone(), method.clone(), *side));
                            }
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
            Some(_) if rooted_in_framework(index, class, class_side, recv.as_ref().unwrap()) => {}
            Some((_, ty)) => {
                // INTERIM (rubys/roundhouse#58, G2/G3): `routes::plan`'s
                // own re-lowering re-types every body from scratch and
                // loses the analyzer's type for values that come from a
                // framework helper (Devise's `current_<scope>`) or the
                // app's own private helpers (pinned `Untyped` on purpose
                // by #810), reading back as the open `Var`/`Bottom`
                // sentinel or `Untyped`. Only in that case, fall back to
                // the analyzer's own stamp for this exact receiver span
                // (`lowered.controllers`, never re-lowered) — but only
                // when the analyzer itself considers it resolved, never
                // a `Var`/`Bottom`/`Untyped` of its own and never a
                // union with an unresolved member, so a wrong guess can
                // never make the planner skip a real dispatch target.
                // Remove this once G2/G3 are settled upstream.
                let needs_fallback = ty.is_none_or(|t| t.is_open() || t.is_unknown());
                let classes = if needs_fallback {
                    let r = recv.as_ref().unwrap();
                    index
                        .analyzer_stamps
                        .get(&(r.span.file.0, r.span.start, r.span.end))
                        .filter(|t| !t.is_open() && !t.is_unknown() && !t.mentions_unknown())
                        .map(classes_of)
                        .unwrap_or_else(|| ty.map(classes_of).unwrap_or_default())
                } else {
                    ty.map(classes_of).unwrap_or_default()
                };
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

/// Is `e` a chain of sends rooted in a constant the app does not define,
/// or in a receiverless call `class` itself does not resolve? `class` and
/// `class_side` are the method body being walked, so a receiverless call
/// is checked the same way `enqueue_send`'s own dispatch is: through
/// `class`'s lookup, never by asking whether some unrelated app class
/// happens to define a method of that name.
fn rooted_in_framework(index: &Index, class: &str, class_side: bool, e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Const { path } => {
            let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            !index.classes.contains_key(&name)
        }
        ExprNode::Send { recv: Some(r), .. } => rooted_in_framework(index, class, class_side, r),
        // `request.env`, `params[:x]`, `cookies`: a receiverless call
        // `class`'s own lookup misses is the framework's accessor.
        ExprNode::Send { recv: None, method, .. } => index.lookup(class, method, class_side).is_none(),
        _ => false,
    }
}

/// INTERIM (see `enqueue_send`'s fallback, rubys/roundhouse#58): every
/// expression's type as the analyzer itself left it on `app.controllers`
/// — the app's own action and private-helper bodies — keyed by exact
/// source span. `routes::plan` re-lowers these same bodies into fresh
/// `LibraryClass`es through `emit::ruby::lowered_controller_lcs_with_layout`,
/// which reads `app.controllers` by shared reference and re-types a
/// *clone* of each body from scratch (`lower::typing::type_method_body`);
/// it never mutates `app.controllers` itself, so the analyzer's original
/// stamps survive here untouched. The two trees share spans because the
/// lowering only re-types in place — it does not move or resynthesize
/// expression spans — so a span found here names the same source
/// location as its counterpart in the re-lowered body.
fn analyzer_stamps(app: &App) -> HashMap<(u32, u32, u32), Ty> {
    let mut out: HashMap<(u32, u32, u32), Ty> = HashMap::new();
    // A span can recur with a different type, and not every recurrence
    // is a real disagreement:
    //
    //   * safe navigation (`u&.email`) re-types its guarded receiver
    //     TWICE at the identical span — once with its honest, unnarrowed
    //     type (`T | nil`, the local's static type) and once narrowed to
    //     plain `T` inside the branch where the dispatch actually runs.
    //     `classes_of` already drops a union's `Nil` member, so these
    //     two answer `enqueue_send`'s question identically; keeping
    //     either is fine, and `dispatch_classes` below treats them as
    //     the same stamp rather than a conflict.
    //   * a synthesized method body can share its span with another
    //     (`owns_source`'s note on synthesized accessors borrowing
    //     spans), and a concern spliced into several controllers
    //     replays the same source at the same span under a different
    //     `self_ty` each time — these CAN genuinely disagree on which
    //     app classes a call reaches, and that is a real conflict: the
    //     first type seen is no more trustworthy than the second, so a
    //     span whose dispatch classes ever disagree is dropped for good
    //     rather than left holding whichever type happened to visit
    //     first — a wrong stamp here would narrow `enqueue_send`'s
    //     dispatch past a real target.
    //
    // An open or unknown type (`Var`/`Bottom`/`Untyped`, or a union
    // mentioning one) is never recorded and never conflicts: `enqueue_send`
    // already refuses to adopt one (see its own filter), so storing it
    // would only ever risk manufacturing a conflict against a later,
    // useful closed stamp for no benefit.
    let mut conflicted: HashSet<(u32, u32, u32)> = HashSet::new();
    let mut record = |e: &Expr| {
        let Some(ty) = &e.ty else { return };
        if e.span.file.0 == 0 || ty.is_open() || ty.is_unknown() || ty.mentions_unknown() {
            return;
        }
        let key = (e.span.file.0, e.span.start, e.span.end);
        if conflicted.contains(&key) {
            return;
        }
        match out.get(&key) {
            None => {
                out.insert(key, ty.clone());
            }
            Some(existing) if existing == ty => {}
            Some(existing) if dispatch_classes(existing) == dispatch_classes(ty) => {}
            Some(_) => {
                out.remove(&key);
                conflicted.insert(key);
            }
        }
    };
    for c in &app.controllers {
        for item in &c.body {
            match item {
                ControllerBodyItem::Action { action, .. } => visit(&action.body, &mut record),
                ControllerBodyItem::ClassMethod { method, .. } => visit(&method.body, &mut record),
                _ => {}
            }
        }
    }
    out
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

/// `classes_of` as an order-insensitive set, for comparing whether two
/// types answer `enqueue_send`'s dispatch question the same way —
/// `analyzer_stamps`'s own conflict check, so a nilable union and its
/// narrowed non-nil member (safe navigation's two stamps for one
/// receiver span) compare equal without caring which union variant
/// order either happened to carry.
fn dispatch_classes(ty: &Ty) -> BTreeSet<String> {
    classes_of(ty).into_iter().collect()
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
    use super::{analyzer_stamps, app_relative, gap_blocking, may_overlap, view_prefix};
    use crate::dialect::{Action, Controller, ControllerBodyItem, LayoutDecl, RenderTarget};
    use crate::expr::{Expr, ExprNode};
    use crate::ident::{ClassId, Symbol, TyVar};
    use crate::span::{FileId, Span};
    use crate::ty::{Row, Ty};
    use crate::App;

    /// A minimal controller holding one action whose whole body is `e`,
    /// so `analyzer_stamps` sees exactly the spans and types `e` carries.
    fn controller_with_action(class: &str, action: &str, body: Expr) -> Controller {
        Controller {
            name: ClassId(Symbol::from(class)),
            parent: None,
            parent_span: Span::synthetic(),
            body: vec![ControllerBodyItem::Action {
                action: Action {
                    name: Symbol::from(action),
                    params: Row::closed(),
                    opt_params: Vec::new(),
                    kw_params: Vec::new(),
                    kwrest_param: None,
                    block_param: None,
                    rest_param: None,
                    anonymous_formal: None,
                    name_span: Span::synthetic(),
                    body,
                    renders: RenderTarget::Inferred,
                    effects: Default::default(),
                },
                leading_comments: Vec::new(),
                leading_blank_line: false,
            }],
            layout: LayoutDecl::Inherit,
            sibling_classes: Vec::new(),
        }
    }

    fn class_ty(name: &str) -> Ty {
        Ty::Class { id: ClassId(Symbol::from(name)), args: Vec::new().into() }
    }

    fn stamped(span: Span, ty: Ty) -> Expr {
        let mut e = Expr::new(span, ExprNode::SelfRef);
        e.ty = Some(ty);
        e
    }

    /// Two controllers whose bodies were spliced from the same concern
    /// source (`ingest::splice_concerns_into_controllers` copies a
    /// concern's methods into each includer) can carry the SAME span
    /// typed differently per includer — e.g. a private `target` helper
    /// the concern calls answers a different model in each controller.
    /// The first type seen for that span is no more trustworthy than the
    /// second, so a span that ever disagrees with itself must yield no
    /// stamp at all, never whichever type happened to visit first: a
    /// wrong stamp would let `enqueue_send` narrow dispatch past a real
    /// target.
    #[test]
    fn a_span_with_two_different_types_yields_no_stamp() {
        let conflict_span = Span { file: FileId(1), start: 10, end: 20 };
        let app = App {
            controllers: vec![
                controller_with_action("WidgetsController", "index", stamped(conflict_span, class_ty("Widget"))),
                controller_with_action("ThingsController", "index", stamped(conflict_span, class_ty("Thing"))),
            ],
            ..Default::default()
        };
        let stamps = analyzer_stamps(&app);
        assert_eq!(stamps.get(&(1, 10, 20)), None, "{stamps:?}");
    }

    /// The same span typed the SAME way twice (two includers whose
    /// helper happens to answer the same model) is not a conflict: the
    /// stamp survives.
    #[test]
    fn a_span_with_the_same_type_twice_keeps_its_stamp() {
        let agree_span = Span { file: FileId(1), start: 30, end: 40 };
        let app = App {
            controllers: vec![
                controller_with_action("WidgetsController", "index", stamped(agree_span, class_ty("Widget"))),
                controller_with_action("GadgetsController", "index", stamped(agree_span, class_ty("Widget"))),
            ],
            ..Default::default()
        };
        let stamps = analyzer_stamps(&app);
        assert_eq!(stamps.get(&(1, 30, 40)), Some(&class_ty("Widget")));
    }

    fn nilable_class_ty(name: &str) -> Ty {
        Ty::Union { variants: vec![class_ty(name), Ty::Nil].into() }
    }

    /// Safe navigation (`u&.email`) re-types its guarded receiver TWICE
    /// at the identical span: its honest, unnarrowed type (`T | nil`)
    /// and, in the non-nil dispatch branch, plain `T`. This is the
    /// actual shape that regressed rubys/roundhouse#1001 — neither side
    /// is open or unknown, so "ignore open/unknown types" alone does not
    /// cover it. `classes_of` already drops a union's `Nil` member, so
    /// both stamps answer `enqueue_send`'s dispatch question identically
    /// and must not be treated as a conflict.
    #[test]
    fn a_nilable_union_and_its_narrowed_non_nil_member_are_not_a_conflict() {
        let span = Span { file: FileId(1), start: 50, end: 60 };
        let app = App {
            controllers: vec![controller_with_action(
                "WidgetsController",
                "index",
                // One occurrence narrowed, one not — order must not
                // matter, so this covers unnarrowed-then-narrowed.
                Expr::new(Span::synthetic(), ExprNode::Seq {
                    exprs: vec![
                        stamped(span, nilable_class_ty("Widget")),
                        stamped(span, class_ty("Widget")),
                    ],
                }),
            )],
            ..Default::default()
        };
        let stamps = analyzer_stamps(&app);
        assert!(stamps.get(&(1, 50, 60)).is_some(), "{stamps:?}");
    }

    /// An open type (`Var`, the analyzer's own "not yet inferred"
    /// sentinel) recorded before a later, closed type for the same span
    /// must not poison it: the open occurrence is never stored in the
    /// first place, so the closed one that follows is simply the span's
    /// first real entry.
    #[test]
    fn an_open_type_then_a_closed_type_keeps_the_closed_one() {
        let span = Span { file: FileId(1), start: 70, end: 80 };
        let open = Ty::Var { var: TyVar(0) };
        let app = App {
            controllers: vec![controller_with_action(
                "WidgetsController",
                "index",
                Expr::new(Span::synthetic(), ExprNode::Seq {
                    exprs: vec![stamped(span, open), stamped(span, class_ty("Widget"))],
                }),
            )],
            ..Default::default()
        };
        let stamps = analyzer_stamps(&app);
        assert_eq!(stamps.get(&(1, 70, 80)), Some(&class_ty("Widget")), "{stamps:?}");
    }

    #[test]
    fn overlap_is_segmentwise() {
        assert!(may_overlap("/domains/:id", "/domains/:name"));
        assert!(may_overlap("/domains/:id", "/domains/new"));
        assert!(!may_overlap("/domains/:id", "/domain/:id"));
        assert!(!may_overlap("/domains/:id/edit", "/domains/:id"));
        assert!(may_overlap("/files/*path", "/anything/at/all"));
    }

    /// `IngestError::Unsupported.file` is `Path::display()` of wherever
    /// the survey walked; on Windows that's backslash-separated, and
    /// `app_relative`'s `/app/` match (and `view_prefix`'s `/`-joined
    /// prefix) must still find it there.
    #[test]
    fn app_relative_normalizes_windows_separators() {
        assert_eq!(
            app_relative(r"C:\ci\fixture\app\controllers\widgets_controller.rb"),
            "app/controllers/widgets_controller.rb"
        );
        assert_eq!(
            app_relative(r"C:\ci\fixture\app\views\gadget_styles\show.css.erb"),
            "app/views/gadget_styles/show.css.erb"
        );
    }

    /// A gap recorded with a Windows path must still block the route
    /// whose conventional view it names — this is `gap_blocking`'s own
    /// match, downstream of `app_relative`.
    #[test]
    fn gap_blocking_matches_a_windows_style_view_path() {
        let gaps = vec![(
            r"C:\ci\fixture\app\views\gadget_styles\show.css.erb".to_string(),
            "view template not ingested: erb".to_string(),
        )];
        let prefix = view_prefix("GadgetStylesController", "show");
        assert!(gap_blocking(&gaps, &std::collections::HashSet::new(), &prefix).is_some());
    }
}
