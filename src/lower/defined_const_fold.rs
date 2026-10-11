//! `defined?(Const)` answered at compile time, when the app tree, the
//! lockfile and `config/application.rb` decide it.
//!
//! Real apps guard optional integrations with a constant test:
//! `has_one_attached :favicon if defined?(ActiveStorage)`,
//! `acts_as_tenant :account if defined? ActsAsTenant`,
//! `return unless defined?(Rails) && Rails.respond_to?(:logger)`.
//! The strict targets have no runtime `defined?`, but almost every such
//! guard is decidable from what ingest already holds. The reference
//! program for a target is the EMITTED app, so "defined" means "the
//! emitted program has that constant": a class the tree ingested, a
//! framework the application loads, a modeled gem the Gemfile asks for.
//!
//! The rule, per `defined?(A::B::C)` on a plain (possibly `::`-rooted)
//! constant path. Anything else — a method, ivar, `yield`, `super`,
//! expression — is never touched.
//!
//! TRUE, folded to `"constant"`:
//!  * the path (or, for an unrooted path, its name under the owning
//!    class's lexical namespaces) names a class, module or constant the
//!    tree ingested, or a namespace prefix of one (Zeitwerk's implicit
//!    namespaces);
//!  * `Rails` (and `Rails::Application`'s roots) when
//!    `config/application.rb` requires `rails/...`; a Rails framework
//!    root (`ActiveStorage`, `ActionCable`, ...) the application loads
//!    (`require "rails/all"` or that framework's own railtie/engine
//!    require), and `<Framework>::Base`;
//!  * a gem the Gemfile names directly, from a short catalog of the
//!    constants that gem itself defines (`turbo-rails`:
//!    `Turbo::StreamsChannel`; `solid_queue`: `SolidQueue::Job`, ...).
//!
//! FALSE, folded to `nil`, only when ALL of these hold: a Gemfile.lock
//! was read; the top-level name is in no ingested class/constant
//! (compared segment-wise, so a lexical or inherited resolution cannot
//! hide it); no `.rb` source of the app writes a `class`/`module`/
//! `Name =`/`const_set` for it (initializer-defined constants the tree
//! dropped); it is not a Ruby core/stdlib constant or a Rails framework
//! root (those depend on `require`, so they are left alone); and no gem
//! in the lock — direct OR transitive — could own it (its name,
//! first dash-segment, or a gem name it prefixes, compared
//! case/separator-insensitively: `OmniAuth` ~ `omniauth`, `Refer` ~
//! `referral`).
//!
//! Everything else is left as `defined?`, so the strict target still
//! refuses it. A wrong `false` silently disables app behaviour; a wrong
//! `true` or an unfolded guard stays a diagnostic, so every ambiguous
//! class falls toward leaving the error:
//!  * a nested path whose prefix resolves but whose tail does not is
//!    NOT folded (`SourceMonitor::Nope`): a gem or reopening may add it;
//!  * a gem present in the lock but outside the catalog is not folded
//!    either way (it may be `require: false`, a dev-only group, or only
//!    transitive and so never required);
//!  * with no Gemfile.lock, nothing folds false.
//!
//! The fold also prunes what it decides: a literal condition of an
//! `if`/`unless`/ternary keeps the live branch, `&&`/`||` with a folded
//! left operand keep their value-preserving side, `!` of a folded
//! literal becomes a bool. Only subtrees containing a fold are touched,
//! so the user's own `if true` is left alone. The Ruby target runs this
//! too; the literal is exactly what Ruby's `defined?` returns for a
//! constant that exists/doesn't, so behaviour is unchanged.

use std::collections::HashSet;

use crate::app::App;
use crate::expr::{BoolOpKind, Expr, ExprNode, Literal};
use crate::ident::ClassId;
use crate::ty::Ty;

/// Framework roots, with the `require`s that load each.
const FRAMEWORKS: &[(&str, &[&str])] = &[
    ("ActiveRecord", &["active_record/railtie", "active_record"]),
    ("ActiveStorage", &["active_storage/engine"]),
    ("ActionController", &["action_controller/railtie", "action_controller"]),
    ("ActionView", &["action_view/railtie", "action_view"]),
    ("ActionMailer", &["action_mailer/railtie", "action_mailer"]),
    ("ActiveJob", &["active_job/railtie", "active_job"]),
    ("ActionCable", &["action_cable/engine", "action_cable"]),
    ("ActionMailbox", &["action_mailbox/engine"]),
    ("ActionText", &["action_text/engine"]),
];

/// Always present once `rails` is required.
const BASE_FRAMEWORKS: &[&str] = &["Rails", "ActiveSupport", "ActiveModel", "ActionDispatch"];

/// Roots whose `defined?` depends on a `require` the tree cannot show.
const RUNTIME_PROVIDED: &[&str] = &[
    "Object", "BasicObject", "Kernel", "Module", "Class", "Comparable", "Enumerable", "Integer",
    "Float", "Numeric", "String", "Symbol", "Array", "Hash", "NilClass", "TrueClass",
    "FalseClass", "Time", "Date", "DateTime", "Range", "Regexp", "Proc", "Struct", "Data",
    "Exception", "StandardError", "Set", "JSON", "URI", "Net", "Logger", "Digest", "OpenSSL",
    "SecureRandom", "Timeout", "StringIO", "FileUtils", "Tempfile", "YAML", "Psych", "CSV",
    "ERB", "Benchmark", "Forwardable", "Singleton", "Observable", "Open3", "Shellwords", "Zlib",
    "Socket", "BigDecimal", "Rational", "Complex", "Thread", "Mutex", "Process", "Signal",
    "Marshal", "ObjectSpace", "GC", "File", "Dir", "IO", "Math", "Random", "Encoding", "Errno",
    "Etc", "Monitor", "RbConfig", "Gem", "Bundler", "Pathname", "Ractor", "Fiber", "Method",
    "ENV", "ARGV", "ARGF", "STDIN", "STDOUT", "STDERR", "Rack", "Zeitwerk", "I18n", "Minitest",
    "Rake", "Sprockets", "Thor", "Concurrent", "Racc", "Nokogiri", "Mail", "Marcel", "Rouge",
];

/// Constants a gem itself defines, for gems the app names directly.
const GEM_CONSTANTS: &[(&str, &[&str])] = &[
    ("turbo-rails", &["Turbo", "Turbo::StreamsChannel"]),
    ("solid_cable", &["SolidCable"]),
    ("solid_cache", &["SolidCache"]),
    (
        "solid_queue",
        &[
            "SolidQueue", "SolidQueue::Job", "SolidQueue::Process", "SolidQueue::RecurringTask",
            "SolidQueue::ReadyExecution", "SolidQueue::ScheduledExecution",
            "SolidQueue::FailedExecution", "SolidQueue::ClaimedExecution",
        ],
    ),
];

/// The framework roots `config/application.rb` loads. `None` when the
/// file requires no `rails` at all (not a Rails app, or unreadable).
pub fn loaded_frameworks(source: &str) -> Option<Vec<String>> {
    let requires: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("require "))
        .map(|r| r.trim().trim_matches(|c| c == '"' || c == '\'' || c == '(' || c == ')'))
        .collect();
    let all = requires.contains(&"rails/all");
    if !all && !requires.contains(&"rails") && !requires.iter().any(|r| r.ends_with("/railtie") || r.ends_with("/engine")) {
        return None;
    }
    let mut out: Vec<String> = BASE_FRAMEWORKS.iter().map(|s| s.to_string()).collect();
    for (root, reqs) in FRAMEWORKS {
        if all || reqs.iter().any(|r| requires.contains(r)) {
            out.push((*root).to_string());
        }
    }
    Some(out)
}

struct Facts {
    /// Every ingested class/module/constant, fully qualified, plus every
    /// namespace prefix of one.
    tree_and_prefixes: HashSet<String>,
    /// Every `::`-segment of any tree name.
    segments: HashSet<String>,
    frameworks: Option<Vec<String>>,
    lock: Option<crate::gems::Lockfile>,
    /// `.rb` source text, for names written to be defined somewhere.
    ruby_sources: Vec<String>,
}

impl Facts {
    fn of(app: &App) -> Facts {
        let mut tree: HashSet<String> = HashSet::new();
        for m in &app.models {
            tree.insert(m.name.0.as_str().to_string());
        }
        for c in &app.controllers {
            tree.insert(c.name.0.as_str().to_string());
        }
        for lc in app.library_classes.iter().chain(app.rails_application.iter()) {
            let name = lc.name.0.as_str().to_string();
            for (c, _) in &lc.constants {
                tree.insert(format!("{name}::{}", c.as_str()));
            }
            tree.insert(name);
        }
        let mut tree_and_prefixes = tree.clone();
        let mut segments = HashSet::new();
        for name in &tree {
            let mut acc = String::new();
            for seg in name.split("::") {
                if !acc.is_empty() {
                    acc.push_str("::");
                }
                acc.push_str(seg);
                tree_and_prefixes.insert(acc.clone());
                segments.insert(seg.to_string());
            }
        }
        Facts {
            tree_and_prefixes,
            segments,
            frameworks: app.loaded_frameworks.clone(),
            lock: app.gem_lock.clone(),
            ruby_sources: app
                .sources
                .iter()
                .filter(|s| s.path.ends_with(".rb"))
                .map(|s| s.text.clone())
                .collect(),
        }
    }

    /// `Some(answer)` when decidable; `None` leaves the `defined?`.
    fn decide(&self, owner: Option<&ClassId>, path: &[String]) -> Option<bool> {
        let rooted = path.first().is_some_and(|s| s.is_empty());
        let segs: Vec<&str> = path.iter().filter(|s| !s.is_empty()).map(String::as_str).collect();
        let head = *segs.first()?;
        let full = segs.join("::");

        // The lexical namespaces an unrooted name is looked up under.
        let mut scopes: Vec<String> = Vec::new();
        if !rooted {
            if let Some(owner) = owner {
                let parts: Vec<&str> = owner.0.as_str().split("::").collect();
                for n in (1..=parts.len()).rev() {
                    scopes.push(parts[..n].join("::"));
                }
            }
        }
        // The innermost scope that binds the head decides the lookup.
        let bound_scope = scopes
            .iter()
            .find(|s| self.tree_and_prefixes.contains(&format!("{s}::{head}")))
            .cloned();
        if let Some(scope) = bound_scope {
            let qualified = format!("{scope}::{full}");
            return self.tree_and_prefixes.contains(&qualified).then_some(true);
        }
        if self.tree_and_prefixes.contains(&full) {
            return Some(true);
        }
        if self.tree_and_prefixes.contains(head) {
            // Prefix resolves, tail does not: a gem or a reopening may add it.
            return None;
        }

        if let Some(frameworks) = &self.frameworks {
            if frameworks.iter().any(|f| f == head) {
                return match segs.as_slice() {
                    [_] => Some(true),
                    [_, "Base"] if head != "Rails" => Some(true),
                    _ => None,
                };
            }
        }
        if let Some(lock) = &self.lock {
            if GEM_CONSTANTS
                .iter()
                .any(|(gem, consts)| lock.dependencies.iter().any(|d| d == gem) && lock.has(gem) && consts.contains(&full.as_str()))
            {
                return Some(true);
            }
        }
        self.provably_absent(head).then_some(false)
    }

    fn provably_absent(&self, head: &str) -> bool {
        let Some(lock) = &self.lock else { return false };
        if self.segments.contains(head)
            || RUNTIME_PROVIDED.contains(&head)
            || BASE_FRAMEWORKS.contains(&head)
            || FRAMEWORKS.iter().any(|(root, _)| *root == head)
            || self.written_as_definition(head)
        {
            return false;
        }
        let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        let h = norm(head);
        if h.is_empty() {
            return false;
        }
        !lock.specs.iter().any(|(gem, _)| {
            let g = norm(gem);
            let first = norm(gem.split(['-', '_']).next().unwrap_or(gem));
            g == h || first == h || g.starts_with(&h) || h.starts_with(&g)
                || crate::gems::namespace_of(gem).eq_ignore_ascii_case(head)
        })
    }

    /// Does any app `.rb` source write a definition of `name`
    /// (`class`/`module` line naming it, `Name =`, `const_set`)?
    /// Anywhere at all, deliberately coarse: only ever blocks a `false`.
    fn written_as_definition(&self, name: &str) -> bool {
        let is_ident = |c: char| c.is_alphanumeric() || c == '_';
        self.ruby_sources.iter().any(|text| {
            if !text.contains(name) {
                return false;
            }
            text.lines().any(|line| {
                let t = line.trim_start();
                if t.starts_with('#') {
                    return false;
                }
                let mut from = 0;
                while let Some(i) = line[from..].find(name) {
                    let at = from + i;
                    let before = line[..at].chars().next_back();
                    let rest = &line[at + name.len()..];
                    from = at + name.len();
                    if before.is_some_and(is_ident) || rest.chars().next().is_some_and(is_ident) {
                        continue;
                    }
                    if t.starts_with("class ") || t.starts_with("module ") {
                        return true;
                    }
                    let r = rest.trim_start();
                    if r.starts_with('=') && !r.starts_with("==") && !r.starts_with("=~") && !r.starts_with("=>") {
                        return true;
                    }
                    if r.starts_with("||=") {
                        return true;
                    }
                    if line[..at].contains("const_set") {
                        return true;
                    }
                }
                false
            })
        })
    }
}

pub fn apply_defined_const_fold(app: &mut App) {
    // Nothing to fold in an app that never asks.
    let mut asks = false;
    super::for_each_hook_body_ref(app, &mut |e| asks |= contains_defined(e));
    asks |= app.views.iter().any(|v| contains_defined(&v.body));
    if !asks {
        return;
    }
    let facts = Facts::of(app);
    // Class-body statements (`has_one_attached :x if defined?(Y)`) were
    // classified at ingest, before this pass; a guard that folds to its
    // bare statement is not re-classified (a controller `before_action`
    // would silently never install). See `fold_class_statement`.
    let mut class_stmts: HashSet<usize> = HashSet::new();
    let addr = |e: &Expr| e as *const Expr as usize;
    for m in &app.models {
        for item in &m.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item {
                class_stmts.insert(addr(expr));
            }
        }
    }
    for c in &app.controllers {
        for item in &c.body {
            if let crate::dialect::ControllerBodyItem::Unknown { expr, .. } = item {
                class_stmts.insert(addr(expr));
            }
        }
    }
    for lc in app.library_classes.iter().chain(app.rails_application.iter()) {
        for call in &lc.unknown_calls {
            class_stmts.insert(addr(call));
        }
    }
    super::for_each_owned_hook_body(app, &mut |owner, body| {
        if class_stmts.contains(&(&*body as *const Expr as usize)) {
            fold_class_statement(body, owner, &facts);
        } else {
            walk(body, owner, &facts);
        }
    });
    // Templates belong to no class: an unrooted name is looked up from
    // the top level.
    for view in &mut app.views {
        walk(&mut view.body, None, &facts);
    }
}

/// Class-body macros whose bare form a LATER pass claims from the
/// holding pen (`lower::attached`), so a true guard may be hoisted.
const CLAIMED_AFTER_FOLD: &[&str] = &["has_one_attached", "has_many_attached"];

/// A guarded class-body statement. A false guard drops the statement,
/// which is exactly what Ruby does. A true guard is hoisted only for a
/// macro a later pass claims; any other (`before_action`, `validates`,
/// `devise`, ...) would become a bare call nothing classifies, i.e. a
/// silently missing declaration, so it keeps its `defined?` and stays a
/// refused construct. Guards not at the top of the statement, and
/// statements that are not guards, fold as in any expression.
fn fold_class_statement(e: &mut Expr, owner: Option<&ClassId>, facts: &Facts) {
    let guarded = matches!(&*e.node, ExprNode::If { .. } | ExprNode::BoolOp { .. });
    if !guarded {
        walk(e, owner, facts);
        return;
    }
    let mut folded = e.clone();
    if !walk(&mut folded, owner, facts) {
        return;
    }
    let keep = match &*folded.node {
        ExprNode::Lit { value: Literal::Nil } => true,
        ExprNode::Send { recv: None, method, .. } => CLAIMED_AFTER_FOLD.contains(&method.as_str()),
        _ => false,
    };
    if keep {
        *e = folded;
    }
}

fn contains_defined(e: &Expr) -> bool {
    if matches!(&*e.node, ExprNode::Defined { .. }) {
        return true;
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found |= contains_defined(c));
    found
}

/// Literal truthiness, for the value shapes this pass produces.
fn truth(e: &Expr) -> Option<bool> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Nil } => Some(false),
        ExprNode::Lit { value: Literal::Bool { value } } => Some(*value),
        ExprNode::Lit { value: Literal::Str { .. } } => Some(true),
        _ => None,
    }
}

fn lit(like: &Expr, value: Literal, ty: Ty) -> Expr {
    let mut out = Expr::new(like.span, ExprNode::Lit { value });
    out.ty = Some(ty);
    out
}

/// Fold `e` in place; true when anything under it changed.
fn walk(e: &mut Expr, owner: Option<&ClassId>, facts: &Facts) -> bool {
    let mut changed: Vec<bool> = Vec::new();
    e.node.for_each_child_mut(&mut |c| changed.push(walk(c, owner, facts)));
    let any = changed.iter().any(|c| *c);

    if let ExprNode::Defined { operand } = &*e.node {
        if let ExprNode::Const { path } = &*operand.node {
            let path: Vec<String> = path.iter().map(|s| s.as_str().to_string()).collect();
            let answer = facts.decide(owner, &path);
            match answer {
                Some(true) => {
                    *e = lit(e, Literal::Str { value: "constant".into() }, Ty::Str);
                    return true;
                }
                Some(false) => {
                    *e = lit(e, Literal::Nil, Ty::Nil);
                    return true;
                }
                None => return any,
            }
        }
        return any;
    }
    if !any {
        return false;
    }

    // `changed[i]`: did child i hold a fold. Only then is a literal
    // there this pass's doing.
    let node = std::mem::replace(&mut *e.node, ExprNode::Lit { value: Literal::Nil });
    let (new_node, replacement): (ExprNode, Option<Expr>) = match node {
        ExprNode::If { cond, then_branch, else_branch } if changed[0] => match truth(&cond) {
            Some(true) => (ExprNode::Lit { value: Literal::Nil }, Some(then_branch)),
            Some(false) => (ExprNode::Lit { value: Literal::Nil }, Some(else_branch)),
            None => (ExprNode::If { cond, then_branch, else_branch }, None),
        },
        ExprNode::BoolOp { op, surface, left, right } if changed[0] => match (truth(&left), op) {
            (Some(true), BoolOpKind::And) | (Some(false), BoolOpKind::Or) => {
                (ExprNode::Lit { value: Literal::Nil }, Some(right))
            }
            (Some(false), BoolOpKind::And) | (Some(true), BoolOpKind::Or) => {
                (ExprNode::Lit { value: Literal::Nil }, Some(left))
            }
            (None, _) => (ExprNode::BoolOp { op, surface, left, right }, None),
        },
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "!" && args.is_empty() && changed[0] && truth(&r).is_some() =>
        {
            let v = !truth(&r).unwrap();
            (ExprNode::Lit { value: Literal::Bool { value: v } }, None)
        }
        other => (other, None),
    };
    match replacement {
        Some(r) => *e = r,
        None => {
            *e.node = new_node;
            if matches!(&*e.node, ExprNode::Lit { value: Literal::Bool { .. } }) {
                e.ty = Some(Ty::Bool);
            }
        }
    }
    true
}
