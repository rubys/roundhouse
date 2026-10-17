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
//! whose receiver the analysis did not type reaches that method on every
//! class that can have an instance while the route runs (rapid type
//! analysis; see `Walk`), or on every app class when the route takes a
//! class or method name from data, and a symbol literal naming an app
//! method counts as a call (callbacks, `send`). An error inside a
//! reached method's source range taints the route; an error outside
//! every method (a class-level `include`) taints routes that reach any
//! method in its file.
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
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal, Pattern};
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
    /// Last segment of a class name -> every class so named, for
    /// resolving a constant whose path is written relative to its scope.
    by_last: HashMap<String, Vec<String>>,
    /// class -> its direct subclasses.
    children: HashMap<String, Vec<String>>,
    /// class -> itself and everything above it (parents and included
    /// modules, transitively), app or not.
    ancestors: HashMap<String, HashSet<String>>,
    /// INTERIM (see `enqueue_send`'s fallback): the analyzer's own type
    /// for every expression in the app's controller bodies, from before
    /// `routes::plan`'s own re-lowering re-types them from scratch and
    /// loses framework/private-helper receiver types (G2/G3, tracked
    /// upstream as rubys/roundhouse#58). Keyed by exact source span
    /// (file, start, end), since the re-lowering reads the same spans
    /// without moving them.
    analyzer_stamps: HashMap<(u32, u32, u32), Ty>,
    /// Every class whose instance-side lookup ends in a `method_missing`
    /// the app wrote, with that method.
    missing: Vec<(String, MethodKey)>,
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
        let mut by_last: HashMap<String, Vec<String>> = HashMap::new();
        for name in classes.keys() {
            by_last.entry(name.rsplit("::").next().unwrap_or(name).to_string()).or_default().push(name.clone());
        }
        let mut index = Index {
            classes,
            by_name,
            by_last,
            children: HashMap::new(),
            ancestors: HashMap::new(),
            analyzer_stamps,
            missing: Vec::new(),
        };
        // A parent or include is written as the source spelled it, which
        // may be relative to the enclosing namespace: take every app class
        // it could name (more ancestors is the safe direction), and the
        // name itself, which is how a framework base stays visible.
        let mut up: HashMap<String, Vec<String>> = HashMap::new();
        for (name, lcs) in &index.classes {
            let mut out = Vec::new();
            for lc in lcs {
                for p in lc.includes.iter().chain(lc.parent.iter()) {
                    let p = p.0.as_str();
                    out.push(p.to_string());
                    out.extend(index.app_classes_named_str(p));
                }
                if let Some(p) = &lc.parent {
                    for c in std::iter::once(p.0.as_str().to_string()).chain(index.app_classes_named_str(p.0.as_str())) {
                        index.children.entry(c).or_default().push(name.clone());
                    }
                }
            }
            up.insert(name.clone(), out);
        }
        for list in index.children.values_mut() {
            list.sort();
            list.dedup();
        }
        for name in index.classes.keys() {
            let mut seen: HashSet<String> = HashSet::new();
            let mut stack = vec![name.clone()];
            while let Some(c) = stack.pop() {
                if seen.insert(c.clone()) {
                    stack.extend(up.get(&c).into_iter().flatten().cloned());
                }
            }
            index.ancestors.insert(name.clone(), seen);
        }
        let mut missing: Vec<(String, MethodKey)> = index
            .classes
            .keys()
            .filter_map(|c| index.lookup(c, &Symbol::from("method_missing"), false).map(|k| (c.clone(), k)))
            .collect();
        missing.sort();
        index.missing = missing;
        index
    }

    /// `class` and every class below it.
    fn tree(&self, class: &str) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack = vec![class.to_string()];
        while let Some(c) = stack.pop() {
            if seen.insert(c.clone()) {
                stack.extend(self.children.get(&c).into_iter().flatten().cloned());
            }
        }
        let mut out: Vec<String> = seen.into_iter().filter(|c| self.classes.contains_key(c)).collect();
        out.sort();
        out
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

    /// Every app class a constant path can name: the path itself, or a
    /// class nested somewhere under that name (`Formatter` written
    /// inside `module Admin` is `Admin::Formatter`). Over-approximates:
    /// that is the safe direction for reach.
    fn app_classes_named(&self, path: &[Symbol]) -> Vec<String> {
        self.app_classes_named_str(&const_name(path))
    }

    fn app_classes_named_str(&self, name: &str) -> Vec<String> {
        let name = name.trim_start_matches("::");
        let last = name.rsplit("::").next().unwrap_or(name);
        let suffix = format!("::{name}");
        self.by_last
            .get(last)
            .into_iter()
            .flatten()
            .filter(|c| c.as_str() == name || c.ends_with(&suffix))
            .cloned()
            .collect()
    }
}

fn const_name(path: &[Symbol]) -> String {
    path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")
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
    let models: Vec<String> = lowered.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    let mut facts = Facts::new(&index, &lcs, &models);
    // Every method's summary, once: their `live` sets bound what shared
    // state can hold.
    let mut summaries: HashMap<MethodKey, Summary> = HashMap::new();
    for lc in &lcs {
        for m in &lc.methods {
            let key = (lc.name.0.as_str().to_string(), m.name.clone(), is_class_side(&m.receiver));
            if !summaries.contains_key(&key) {
                let s = summarize(&index, &facts, &key);
                summaries.insert(key, s);
            }
        }
    }
    let mut app_live: Vec<String> = facts.always_live.clone();
    app_live.extend(controllers.iter().filter(|c| index.classes.contains_key(*c)).cloned());
    app_live.extend(helper_modules(&index));
    app_live.extend(summaries.values().flat_map(|s| s.live.iter().cloned()));
    for root in summaries.values().flat_map(|s| s.live_tree.iter()) {
        app_live.extend(index.tree(root));
    }
    app_live.sort();
    app_live.dedup();
    facts.app_live = app_live;
    // `ROUNDHOUSE_FUTAMURA_LEDGER=2`: each error and the methods owning
    // it, and how each route's walk bounded its untyped sends.
    let ledger2 = std::env::var("ROUNDHOUSE_FUTAMURA_LEDGER").as_deref() == Ok("2");
    if ledger2 {
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
        let Reach { reached, shared, full } = reach(&index, &facts, &mut summaries, controller, first.action.as_str());
        if ledger2 {
            match (&full, &shared) {
                (Some(why), _) => eprintln!("  rta         {key}: full fan-out: {why}"),
                (None, Some(why)) => eprintln!("  rta         {key}: every app-live class: {why}"),
                (None, None) => eprintln!("  rta         {key}: bounded"),
            }
        }
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

// ---------------------------------------------------------------------
// Rapid type analysis (RTA).
//
// A send whose receiver the analysis did not type used to reach every
// app method of that name ("fan-out"). On a large real app that sends
// almost every route to almost every file. RTA narrows it: an untyped
// `x.foo` reaches `foo` only on the classes that can have an instance
// while the route runs, the route's LIVE classes. Reaching a method can
// make more classes live, which can reach more methods, so the walk is
// a fixpoint over both sets.
//
// Soundness. The walk must over-approximate what the box can run: if
// at runtime `x` holds an instance of app class K, then K's `foo` must
// be reached. Every object in the box comes from one of:
//
//   1. app code reached on this route: `K.new` (and any other use of
//      the class object, since whoever holds `K` can call `new` on it),
//      a receiverless `new` in a class-side method of `K`, a value the
//      analysis typed as `K`. Each makes `K` live (`Summary::live`). A
//      subclass `S` of `K` is not made live by these: `S.new`, or an
//      inherited class method called on `S`, needs the `S` class object,
//      which only a reference to `S` (live), an `S` instance's `.class`
//      (live already) or reflection gives. Reflection that lists
//      subclasses (`K.subclasses`, `K.descendants`) makes the whole
//      tree under `K` live (`Summary::live_tree`).
//   2. the runtime. Outside test support it never turns a name into an
//      app class (no `constantize`, no `Object.const_get`), except in the
//      per-app sections `project` generates into it: the
//      `instantiate-named` model table, the job registry, GlobalID
//      lookups. So every model, job and mailer class is live on every
//      route (`always_live`), and anything else the runtime builds
//      comes from a class object case 1 already made live. The
//      dispatcher builds the controller, and Rails mixes every helper
//      module into the view context: both are live too.
//   3. load time: constants, class-level initializers and class-body
//      calls run once, before any request. Every app class they name
//      is live on every route.
//   4. state shared across requests: class-level instance variables,
//      `@@class` and `$global` variables, `Thread.current`, the cache,
//      an app constant holding a built object or one the app changes in
//      place, `instance_variable_get`. A value read from one of these
//      may have been built on another route, by code this route never
//      runs. It was still built by the box, so its class is one that
//      some code in the app makes live (cases 1–3 over every method,
//      not just this route's: `Facts::app_live`). Reaching one makes
//      all of those live for the route (`Walk::shared`).
//
//      The cache is the one case this is only an assumption for: the
//      box's `Rails.cache` is the *same* store Rails itself reads and
//      writes, for every box and every non-box route in the process
//      (and, with a shared cache store, every process). Bounding a
//      cache read to `app_live` assumes the box's cache only ever holds
//      values *some* box code wrote — true on this target's current
//      fixtures, but not something this walk can see from the box's
//      code alone: Rails code elsewhere, or a value a migration or a
//      console session wrote directly, is invisible here, and its class
//      need not be `app_live` at all. So a cache read is CONSERVATIVE
//      by default: full fan-out, case 5 below, not bounded by
//      `app_live`. The bounded assumption is available behind
//      `ROUNDHOUSE_FUTAMURA_TRUST_BOX_CACHE=1` (`Facts::trust_box_cache`)
//      for anyone who has established it holds for their app; see the
//      design question in this PR's description before trusting it.
//   5. anything that turns data into a class or a method name:
//      `constantize`, `const_get`, `Marshal.load`, `YAML.load`,
//      `JSON.load`, `send`/`public_send`/`try` with a computed name,
//      `instance_eval`, `superclass`, … Nothing bounds these, so
//      reaching one puts the route back on full fan-out (`Walk::full`):
//      every untyped name reaches every app method so named, as it did
//      before RTA. (A computed method name is not followed at all, as
//      before; full fan-out only keeps the classes unbounded.)
//
// A live class answers a name through its whole ancestry (parents and
// included modules), so `dispatch` reaches every definer there, not
// just the first: a module's `super`, a prepend, an override all stay
// covered. If no ancestor defines the name, the call lands on a
// `method_missing` in the ancestry, if any.
//
// A send on the result of a framework call (`current_user.account`,
// `request.env.foo`) is an untyped send like any other: a framework call
// can return an app object (a record, the signed-in user). Before RTA
// the planner skipped these chains, since fanning them out by name would
// have reached nearly everything; with RTA they are cheap to follow.
// ---------------------------------------------------------------------

/// What one method body contributes to a route. It does not depend on
/// the route that reached the method, so `plan` computes it once.
#[derive(Default)]
struct Summary {
    /// Calls the walk resolves without liveness: self, constants, typed
    /// receivers, `super`, callbacks named by symbol.
    exact: Vec<MethodKey>,
    /// Names sent to a receiver with no app type: dispatched over the
    /// route's live classes.
    untyped: Vec<Symbol>,
    /// App classes this body makes live.
    live: Vec<String>,
    /// App classes this body makes live with every class below them.
    live_tree: Vec<String>,
    /// The first read of state another request may have written, if any
    /// (case 4 above).
    shared: Option<String>,
    /// The first place a class or method name comes from data, if any
    /// (case 5 above).
    unbounded: Option<String>,
}

/// Whole-app facts every route's walk starts from.
#[derive(Default)]
struct Facts {
    /// Classes live on every route: models, jobs and mailers (the
    /// runtime's generated tables build them by name), and every app
    /// class load-time code names.
    always_live: Vec<String>,
    /// App constants that are values, not classes, by last segment:
    /// a send on one dispatches by name.
    value_consts: HashSet<String>,
    /// Constants whose value can hold an object another request built,
    /// by last segment, with the reason.
    unbounded_consts: HashMap<String, String>,
    /// Every class some code in the box can make live: the union of
    /// every method's `Summary::live`, `always_live`, every controller
    /// and every helper module. Whatever a request left in shared state
    /// is an instance of one of these.
    app_live: Vec<String>,
    /// `ROUNDHOUSE_FUTAMURA_TRUST_BOX_CACHE=1`: trust that this box's
    /// `Rails.cache` only ever holds values some box code wrote, so a
    /// cache read can be bounded to `app_live` (case 4) instead of
    /// falling back to full fan-out (case 5, the default — see the
    /// design-doc comment above `reach`). OFF by default: nothing in
    /// the box's own code can confirm the assumption.
    trust_box_cache: bool,
}

/// Framework bases whose subclasses the runtime's generated sections
/// instantiate by name.
const RUNTIME_BUILT_BASES: &[&str] = &["ActiveRecord::Base", "ActiveJob::Base", "ActionMailer::Base"];

/// Methods that turn data into a class, a method name or code: nothing
/// bounds what they reach (case 5).
const UNBOUNDED_METHODS: &[&str] = &[
    "constantize",
    "safe_constantize",
    "const_get",
    "const_missing",
    "class_eval",
    "module_eval",
    "class_exec",
    "module_exec",
    "instance_eval",
    "instance_exec",
    "eval",
    "binding",
    "superclass",
    "ancestors",
    "_load",
];

/// Methods that read another object's state by name: it may have been
/// written on another request (case 4).
const SHARED_METHODS: &[&str] = &["instance_variable_get", "class_variable_get", "instance_variables"];

/// Methods that change their receiver in place.
fn is_mutator(m: &str) -> bool {
    matches!(
        m,
        "<<" | "push" | "append" | "unshift" | "prepend" | "insert" | "concat" | "[]=" | "store" | "update"
            | "replace" | "add" | "add?" | "clear" | "delete" | "delete_if" | "keep_if" | "shift" | "pop"
            | "fill" | "merge!" | "compare_by_identity" | "instance_variable_set" | "define_singleton_method"
            | "extend"
    ) || m.ends_with('!')
        || (m.ends_with('=') && !matches!(m, "==" | "===" | "!=" | "<=" | ">="))
}

/// `@@x` and `$x`, the variables every request shares; `$1`, `$~` and
/// the other match variables hold Strings and MatchData only.
fn is_shared_var(name: &str) -> bool {
    if name.starts_with("@@") {
        return true;
    }
    let Some(rest) = name.strip_prefix('$') else { return false };
    !(rest.chars().all(|c| c.is_ascii_digit())
        || matches!(
            rest,
            "~" | "&" | "`" | "'" | "+" | "stdout" | "stderr" | "stdin" | "0" | "PROGRAM_NAME" | "LAST_MATCH_INFO"
                | "MATCH" | "PREMATCH" | "POSTMATCH" | "LAST_PAREN_MATCH"
        ))
}

fn first_arg_is_literal(args: &[Expr]) -> Option<Symbol> {
    match args.first().map(|a| &*a.node) {
        Some(ExprNode::Lit { value: Literal::Sym { value } }) => Some(value.clone()),
        Some(ExprNode::Lit { value: Literal::Str { value } }) => Some(Symbol::from(value.as_str())),
        _ => None,
    }
}

fn has_kwarg(args: &[Expr], names: &[&str]) -> bool {
    args.iter().any(|a| match &*a.node {
        ExprNode::Hash { entries, .. } => entries.iter().any(|(k, _)| {
            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if names.contains(&value.as_str()))
        }),
        _ => false,
    })
}

/// Is `e` a non-app constant? Then a send on it is the framework's own
/// class method.
fn framework_const(index: &Index, e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Const { path } if index.app_classes_named(path).is_empty() => Some(const_name(path)),
        _ => None,
    }
}

impl Facts {
    fn new(index: &Index, lcs: &[LibraryClass], models: &[String]) -> Self {
        let mut facts = Facts::default();
        let mut always: Vec<String> = models.iter().filter(|m| index.classes.contains_key(*m)).cloned().collect();
        for (name, anc) in &index.ancestors {
            if !index.is_module(name) && RUNTIME_BUILT_BASES.iter().any(|b| anc.contains(*b)) {
                always.push(name.clone());
            }
        }
        // Load time: constants, class-level initializers, class-body calls.
        // A class body runs with the class itself as `self`: one that
        // builds an instance (`register(new)`), passes `self` on as a
        // value (`register(self)`), or calls an app class method (whose
        // body may keep `self`) can hand the class object, or an
        // instance of it, to anyone, so the class is live. A module body
        // (a concern's `included do … end` runs on the includer) makes
        // every class under the module live.
        for lc in lcs {
            let own = lc.name.0.as_str();
            for e in load_time(lc) {
                if escapes_self(index, own, e) {
                    if lc.is_module {
                        always.extend(index.tree(own).into_iter().filter(|t| t != own && !index.is_module(t)));
                    } else {
                        always.push(own.to_string());
                    }
                }
                visit(e, &mut |n| {
                    if let Some(ty) = &n.ty {
                        always.extend(classes_of(ty).into_iter().filter(|c| index.classes.contains_key(c)));
                    }
                    if let ExprNode::Const { path } = &*n.node {
                        always.extend(index.app_classes_named(path).into_iter().filter(|c| !index.is_module(c)));
                    }
                });
            }
        }
        // Ruby hands every subclass to the parent's `inherited` hook, and
        // every class that includes or extends a module to its
        // `included` / `extended` hook, when that class loads, with no
        // expression in the subclass' own body to show for it: an app
        // hook may keep the class object, so the whole tree under its
        // definer is live.
        for hook in ["inherited", "included", "extended"] {
            for (c, class_side) in index.by_name.get(&Symbol::from(hook)).into_iter().flatten() {
                if *class_side {
                    always.extend(index.tree(c).into_iter().filter(|t| t != c && !index.is_module(t)));
                }
            }
        }
        always.sort();
        always.dedup();
        facts.always_live = always;
        // Constants that are values. One whose initializer calls into
        // the app or calls anything receiverless holds an object built
        // at load time, whose state later requests can change.
        for lc in lcs {
            for (name, e) in &lc.constants {
                facts.value_consts.insert(name.as_str().to_string());
                let mut builds = false;
                visit(e, &mut |n| {
                    if let ExprNode::Send { recv, .. } = &*n.node {
                        match recv.as_ref().map(|r| &*r.node) {
                            None | Some(ExprNode::SelfRef) => builds = true,
                            Some(ExprNode::Const { path }) if !index.app_classes_named(path).is_empty() => builds = true,
                            _ => {}
                        }
                    }
                });
                if builds {
                    facts
                        .unbounded_consts
                        .entry(name.as_str().to_string())
                        .or_insert_with(|| format!("constant {} holds an object built at load time", name.as_str()));
                }
            }
        }
        // An app constant the app changes in place, anywhere, may hold
        // an object a request put there.
        let mut changed: Vec<(String, String)> = Vec::new();
        for lc in lcs {
            for e in lc.methods.iter().map(|m| &m.body).chain(load_time(lc)) {
                visit(e, &mut |n| {
                    let (recv, how) = match &*n.node {
                        ExprNode::Send { recv: Some(r), method, .. } if is_mutator(method.as_str()) => {
                            (r, method.as_str().to_string())
                        }
                        ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => match target {
                            LValue::Index { recv, .. } => (recv, String::from("[]=")),
                            LValue::Attr { recv, name } => (recv, format!("{}=", name.as_str())),
                            _ => return,
                        },
                        _ => return,
                    };
                    if let ExprNode::Const { path } = &*recv.node {
                        if index.app_classes_named(path).is_empty() {
                            let last = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                            changed.push((last, how));
                        }
                    }
                });
            }
        }
        for (last, how) in changed {
            // A framework constant's setters (`I18n.locale =`) keep the
            // runtime's own per-request state; only an app constant
            // holding a value can carry an object from one request to
            // the next.
            if !facts.value_consts.contains(&last) {
                continue;
            }
            facts.unbounded_consts.entry(last.clone()).or_insert_with(|| format!("constant {last} is changed by `{how}`"));
        }
        facts.trust_box_cache = std::env::var("ROUNDHOUSE_FUTAMURA_TRUST_BOX_CACHE").as_deref() == Ok("1");
        facts
    }
}

/// `new` / `allocate` on the class whose class-side body is being
/// walked, receiverless or on `self`: an instance of that class (or of
/// the subclass the call came in on).
fn builds_own_instance(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv, method, .. }
        if matches!(method.as_str(), "new" | "allocate")
            && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)))
}

/// Does `e`, in `class`'s body, hand on `self` (the class object, or an
/// instance it builds)? A `new` / `allocate` on it, a receiverless or
/// `self.` call to an app class method (in a module body, any app class
/// method by that name: `self` there may be the includer), or `self`
/// used as a value rather than a receiver. A framework class-body call
/// (`include`, `delegate`, `attr_reader`) keeps nothing app code can
/// read back.
fn escapes_self(index: &Index, class: &str, e: &Expr) -> bool {
    let module = index.is_module(class);
    let (mut selves, mut receivers, mut escapes) = (0usize, 0usize, false);
    visit(e, &mut |n| match &*n.node {
        ExprNode::SelfRef => selves += 1,
        ExprNode::Send { recv, method, .. } if recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef)) => {
            if recv.is_some() {
                receivers += 1;
            }
            let app_method = if module {
                index.by_name.get(method).into_iter().flatten().any(|(_, side)| *side)
            } else {
                index.lookup(class, method, true).is_some()
            };
            if builds_own_instance(n) || app_method {
                escapes = true;
            }
        }
        _ => {}
    });
    escapes || selves > receivers
}

/// The expressions a class runs once, when it loads.
fn load_time(lc: &LibraryClass) -> impl Iterator<Item = &Expr> {
    lc.constants.iter().map(|(_, e)| e).chain(lc.class_ivar_initializers.iter()).chain(lc.unknown_calls.iter())
}

/// Record what `e` makes live, and whether it brings in a value this
/// walk cannot bound. `class` and `class_side` are the body being walked.
fn observe(index: &Index, facts: &Facts, e: &Expr, class: &str, class_side: bool, s: &mut Summary) {
    let unbounded = |why: String, s: &mut Summary| {
        if s.unbounded.is_none() {
            s.unbounded = Some(why);
        }
    };
    let shared = |why: String, s: &mut Summary| {
        if s.shared.is_none() {
            s.shared = Some(why);
        }
    };
    // A typed value: its class is live. (`self` is an object the walk
    // already counts: the method it runs in was reached through it.)
    if let Some(ty) = e.ty.as_ref().filter(|_| !matches!(&*e.node, ExprNode::SelfRef)) {
        s.live.extend(classes_of(ty).into_iter().filter(|c| index.classes.contains_key(c)));
    }
    match &*e.node {
        ExprNode::Const { path } => {
            let named = index.app_classes_named(path);
            if named.is_empty() {
                let last = path.last().map(|p| p.as_str()).unwrap_or("");
                if last == "ObjectSpace" {
                    shared(String::from("ObjectSpace"), s);
                } else if let Some(why) = facts.unbounded_consts.get(last) {
                    shared(why.clone(), s);
                }
            } else {
                s.live.extend(named.into_iter().filter(|c| !index.is_module(c)));
            }
        }
        // `"#{x}"` calls `x.to_s`, which an app class may define.
        ExprNode::StringInterp { parts } => {
            for part in parts {
                if let InterpPart::Expr { expr } = part {
                    if expr.ty.as_ref().is_none_or(|t| classes_of(t).iter().all(|c| c != "\0none")) {
                        s.untyped.push(Symbol::from("to_s"));
                    }
                }
            }
        }
        ExprNode::Ivar { name } if class_side => shared(format!("class-level {}", name.as_str()), s),
        ExprNode::OpAssign { target: LValue::Ivar { name }, .. } if class_side => {
            shared(format!("class-level {}", name.as_str()), s)
        }
        ExprNode::Var { name, .. } | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. }
            if is_shared_var(name.as_str()) =>
        {
            shared(format!("shared variable {}", name.as_str()), s)
        }
        ExprNode::Send { recv, method, args, .. } => {
            let m = method.as_str();
            if class_side && builds_own_instance(e) {
                s.live.push(class.to_string());
            }
            if UNBOUNDED_METHODS.contains(&m) {
                unbounded(format!("`{m}`"), s);
            }
            if SHARED_METHODS.contains(&m) {
                shared(format!("`{m}`"), s);
            }
            if matches!(m, "send" | "public_send" | "__send__" | "try" | "try!") && !args.is_empty() {
                match first_arg_is_literal(args) {
                    // `x.public_send(:foo)` is `x.foo`.
                    Some(name) if recv.as_ref().is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef)) => {
                        s.untyped.push(name)
                    }
                    Some(_) => {}
                    None => unbounded(format!("`{m}` with a computed name"), s),
                }
            }
            if matches!(m, "subclasses" | "descendants") {
                let roots: Vec<String> = match recv.as_ref().map(|r| (&*r.node, r.ty.as_ref())) {
                    Some((ExprNode::Const { path }, _)) => index.app_classes_named(path),
                    Some((_, Some(ty))) => classes_of(ty).into_iter().filter(|c| index.classes.contains_key(c)).collect(),
                    _ => Vec::new(),
                };
                if roots.is_empty() {
                    unbounded(format!("`{m}` on a class this walk does not know"), s);
                }
                s.live_tree.extend(roots);
            }
            if m == "extend" {
                for a in args {
                    if let ExprNode::Const { path } = &*a.node {
                        s.live.extend(index.app_classes_named(path));
                    }
                }
            }
            let Some(r) = recv else { return };
            if crate::lower::rails_cache::is_rails_cache(r) && matches!(m, "read" | "fetch" | "read_multi" | "fetch_multi") {
                // CONSERVATIVE by default (case 4 vs. 5 in the design-doc
                // comment above `reach`): nothing in the box's own code
                // confirms that this process' `Rails.cache` only ever
                // holds values some box code wrote, so a cache read goes
                // to full fan-out unless the box has opted into trusting
                // it with `ROUNDHOUSE_FUTAMURA_TRUST_BOX_CACHE=1`.
                if facts.trust_box_cache {
                    shared(format!("Rails.cache.{m}"), s);
                } else {
                    unbounded(format!("Rails.cache.{m}"), s);
                }
            }
            if let Some(c) = framework_const(index, r) {
                let last = c.rsplit("::").next().unwrap_or("");
                let decodes = match last {
                    "Marshal" => matches!(m, "load" | "restore"),
                    "YAML" | "Psych" => m.contains("load"),
                    "JSON" => matches!(m, "load" | "load_file" | "unsafe_load")
                        || (m.starts_with("parse") && has_kwarg(args, &["create_additions", "object_class", "array_class"])),
                    _ => false,
                };
                if decodes {
                    unbounded(format!("{c}.{m}"), s);
                }
                if matches!(last, "Thread" | "Fiber") && matches!(m, "current" | "[]") || last == "RequestStore" {
                    shared(format!("{c}.{m}"), s);
                }
                // A send on an app constant that holds a value.
                if facts.value_consts.contains(last) {
                    s.untyped.push(method.clone());
                }
            }
        }
        _ => {}
    }
}

/// One route's walk: the methods reached and the classes live so far.
struct Walk<'i, 'a> {
    index: &'i Index<'a>,
    reached: HashMap<MethodKey, Option<MethodKey>>,
    queue: VecDeque<MethodKey>,
    live: BTreeSet<String>,
    /// Every name sent to an untyped receiver so far, with the first
    /// method that sent it.
    names: Vec<(Symbol, MethodKey)>,
    seen_names: HashSet<Symbol>,
    /// Set once the route reads state another request may have written:
    /// every class in `Facts::app_live` is live from then on.
    shared: Option<String>,
    /// Set once the route reaches a class or method name taken from
    /// data: from then on every untyped name fans out to every definer.
    full: Option<String>,
}

impl<'i, 'a> Walk<'i, 'a> {
    fn push(&mut self, k: MethodKey, from: Option<MethodKey>) {
        if !self.reached.contains_key(&k) {
            self.reached.insert(k.clone(), from);
            self.queue.push_back(k);
        }
    }

    /// `name` on an instance of live class `class`: every definer in its
    /// ancestry, else its `method_missing`. Instance side only, as before
    /// RTA: a class object reaches a send as a constant receiver, which
    /// `enqueue_send` resolves exactly.
    fn dispatch(&mut self, class: &str, name: &Symbol, from: &MethodKey) {
        let index = self.index;
        let Some(anc) = index.ancestors.get(class) else { return };
        let defined = |n: &Symbol| -> Vec<MethodKey> {
            index
                .by_name
                .get(n)
                .into_iter()
                .flatten()
                .filter(|(c, side)| !*side && anc.contains(c))
                .map(|(c, _)| (c.clone(), n.clone(), false))
                .collect()
        };
        let mut hits = defined(name);
        if hits.is_empty() {
            hits = defined(&Symbol::from("method_missing"));
        }
        for k in hits {
            self.push(k, Some(from.clone()));
        }
    }

    /// Every definer of `name` on the instance side, app-wide, plus —
    /// exactly as before RTA (`index.missing`) — every class that does
    /// not define `name` but does have its own `method_missing`: full
    /// fan-out never narrows by liveness, so it must still catch a value
    /// of a class this walk never saw built.
    fn fan_out(&mut self, name: &Symbol, from: &MethodKey) {
        let index = self.index;
        for (c, side) in index.by_name.get(name).into_iter().flatten() {
            if !*side {
                self.push((c.clone(), name.clone(), false), Some(from.clone()));
            }
        }
        for (c, mm) in &index.missing {
            if index.lookup(c, name, false).is_none() {
                self.push(mm.clone(), Some(from.clone()));
            }
        }
    }

    fn add_live_tree(&mut self, class: &str) {
        for c in self.index.tree(class) {
            self.add_live(&c);
        }
    }

    fn add_live(&mut self, class: &str) {
        let c = class.to_string();
        if self.live.insert(c.clone()) {
            if self.full.is_none() {
                for i in 0..self.names.len() {
                    let (n, from) = self.names[i].clone();
                    self.dispatch(&c, &n, &from);
                }
            }
        }
    }

    fn add_name(&mut self, name: &Symbol, from: &MethodKey) {
        if !self.seen_names.insert(name.clone()) {
            return;
        }
        self.names.push((name.clone(), from.clone()));
        if self.full.is_some() {
            self.fan_out(name, from);
        } else {
            let live: Vec<String> = self.live.iter().cloned().collect();
            for c in live {
                self.dispatch(&c, name, from);
            }
        }
    }

    fn go_full(&mut self, why: String) {
        if self.full.is_some() {
            return;
        }
        self.full = Some(why);
        for (n, from) in self.names.clone() {
            self.fan_out(&n, &from);
        }
    }

    fn apply(&mut self, facts: &Facts, from: &MethodKey, s: &Summary) {
        for k in &s.exact {
            self.push(k.clone(), Some(from.clone()));
        }
        if let Some(why) = &s.unbounded {
            self.go_full(format!("{why} in {}", key_label(from)));
        }
        if let (Some(why), None) = (&s.shared, &self.shared) {
            self.shared = Some(format!("{why} in {}", key_label(from)));
            for c in &facts.app_live {
                self.add_live(c);
            }
        }
        for c in &s.live {
            self.add_live(c);
        }
        for c in &s.live_tree {
            self.add_live_tree(c);
        }
        for n in &s.untyped {
            self.add_name(n, from);
        }
    }
}

fn key_label((c, n, side): &MethodKey) -> String {
    format!("{c}{}{}", if *side { "." } else { "#" }, n.as_str())
}

/// What `reach` found for one route.
struct Reach {
    reached: HashMap<MethodKey, Option<MethodKey>>,
    /// Why every class the app can make live is live on the route, if it is.
    shared: Option<String>,
    /// Why the route fell back to full fan-out, if it did.
    full: Option<String>,
}

/// Every app method one action can run: its arm of `process_action`
/// (the filters that apply to it, then the action), closed over calls.
fn reach(
    index: &Index,
    facts: &Facts,
    summaries: &mut HashMap<MethodKey, Summary>,
    controller: &str,
    action: &str,
) -> Reach {
    let mut walk = Walk {
        index,
        reached: HashMap::new(),
        queue: VecDeque::new(),
        live: BTreeSet::new(),
        names: Vec::new(),
        seen_names: HashSet::new(),
        shared: None,
        full: None,
    };
    // The dispatcher builds the controller; helper modules are mixed
    // into the view context.
    let mut seeds: Vec<String> = facts.always_live.clone();
    seeds.push(controller.to_string());
    seeds.extend(helper_modules(index));
    let process = Symbol::from("process_action");
    match index.lookup(controller, &process, false) {
        Some(key) => {
            walk.reached.insert(key.clone(), None);
            let mut s = Summary::default();
            let mut seen: HashMap<MethodKey, Option<MethodKey>> = HashMap::new();
            let mut found: VecDeque<MethodKey> = VecDeque::new();
            for m in index.bodies(&key.0, &process, false) {
                calls_for_action(&m.body, action, &mut |e| {
                    enqueue_send(index, facts, e, controller, false, &key, &mut seen, &mut found, &mut s);
                    observe(index, facts, e, controller, false, &mut s);
                });
            }
            s.exact = found.into();
            for c in &seeds {
                walk.add_live(c);
            }
            walk.apply(facts, &key, &s);
        }
        // No dispatcher: the action is the whole route.
        None => {
            for c in &seeds {
                walk.add_live(c);
            }
            if let Some(key) = index.lookup(controller, &Symbol::from(action), false) {
                walk.push(key, None);
            }
        }
    }
    while let Some(from) = walk.queue.pop_front() {
        let s = summaries.entry(from.clone()).or_insert_with(|| summarize(index, facts, &from));
        walk.apply(facts, &from, s);
    }
    Reach { reached: walk.reached, shared: walk.shared, full: walk.full }
}

/// Helper modules: Rails mixes every one into the view context.
fn helper_modules(index: &Index) -> Vec<String> {
    let mut out: Vec<String> = index.classes.keys().filter(|c| index.is_module(c) && c.ends_with("Helper")).cloned().collect();
    out.sort();
    out
}

/// What a method's body may call and make live, in the order the walk
/// meets it. It does not depend on the route that reached the method, so
/// `plan` keeps one answer per method across routes: on a large app
/// nearly every route reaches most of the code, and re-walking it per
/// route was most of the emit.
fn summarize(index: &Index, facts: &Facts, key: &MethodKey) -> Summary {
    let (class, name, class_side) = key;
    let mut s = Summary::default();
    let mut seen: HashMap<MethodKey, Option<MethodKey>> = HashMap::new();
    let mut found: VecDeque<MethodKey> = VecDeque::new();
    for m in index.bodies(class, name, *class_side) {
        let defaults = m.params.iter().filter_map(|p| p.default.as_ref());
        for body in std::iter::once(&m.body).chain(defaults) {
            visit(body, &mut |e| {
                enqueue_send(index, facts, e, class, *class_side, key, &mut seen, &mut found, &mut s);
                observe(index, facts, e, class, *class_side, &mut s);
            });
        }
    }
    s.exact = found.into();
    s
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
    facts: &Facts,
    e: &Expr,
    class: &str,
    class_side: bool,
    from: &MethodKey,
    reached: &mut HashMap<MethodKey, Option<MethodKey>>,
    queue: &mut VecDeque<MethodKey>,
    summary: &mut Summary,
) {
    let mut push = |k: MethodKey| {
        if !reached.contains_key(&k) {
            reached.insert(k.clone(), Some(from.clone()));
            queue.push_back(k);
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
                    // It may also land on the class's own
                    // `method_missing`, when the app wrote one.
                    None => {
                        if let Some(k) = index.lookup(class, &Symbol::from("method_missing"), class_side) {
                            push(k);
                        }
                        for (c, side) in index.by_name.get(method).into_iter().flatten() {
                            if index.is_module(c) {
                                push((c.clone(), method.clone(), *side));
                            }
                        }
                    }
                }
            }
            Some((ExprNode::Const { path }, _)) => {
                // Every app class the path can name (it may be written
                // relative to its namespace), and a class-side
                // `method_missing` for a name none defines
                // (`Setting.some_key`).
                for name in index.app_classes_named(path) {
                    if let Some(k) = index.lookup(&name, method, true) {
                        push(k);
                    } else if method.as_str() == "new" {
                        if let Some(k) = index.lookup(&name, &Symbol::from("initialize"), false) {
                            push(k);
                        }
                    } else if let Some(k) = index.lookup(&name, &Symbol::from("method_missing"), true) {
                        push(k);
                    }
                }
                // An app constant that holds a value (`SPARE =
                // Gadget.new`), not a class: a send on it dispatches on
                // whatever it holds — any class the route's walk makes
                // live. Checked regardless of whether the loop above
                // also matched some namespaced class by the same last
                // segment (`Admin::SPARE`) — the planner cannot tell
                // which one the path actually names, so it reaches
                // through both.
                if path.last().is_some_and(|l| facts.value_consts.contains(l.as_str())) {
                    summary.untyped.push(method.clone());
                }
            }
            // A send on a framework call's result (`current_user.account`,
            // `Rails.cache.read(k).call`, a record out of a relation) is
            // an untyped send like any other: the framework hands back app
            // objects, so it is no reason to skip the call.
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
                    // No app type — a local, a helper's result, a
                    // framework call's result (`current_user.account`,
                    // `Rails.cache.fetch(k).x`): the route's live classes
                    // decide (RTA, above).
                    summary.untyped.push(method.clone());
                } else {
                    for c in classes {
                        // Each side on its own: a class-side hit says
                        // nothing about whether the instance side
                        // defines it too (`Ty::Class` carries both the
                        // class object and an instance of it), so a hit
                        // on one side must never suppress the other
                        // side's own `method_missing`.
                        for side in [false, true] {
                            match index.lookup(&c, method, side) {
                                Some(k) => push(k),
                                None => {
                                    if let Some(k) = index.lookup(&c, &Symbol::from("method_missing"), side) {
                                        push(k);
                                    }
                                }
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
        ExprNode::MethodRef { name, .. } => summary.untyped.push(name.clone()),
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
