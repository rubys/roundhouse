//! View paths: `prepend_view_path`, `append_view_path` and `view_paths =`.
//!
//! Rails finds a template by searching an ORDERED list of directories, the
//! class's view paths, for the name a render asks for (`pages/index`). An
//! app changes that list at boot (`ActionController::Base.prepend_view_path
//! "app/views/themes/next"` in an initializer) or in a class body
//! (`append_view_path` in one controller, `self.view_paths = [...]` in
//! another), and a template under
//! `app/views/themes/next/pages/index.html.erb` is then what
//! `PagesController#index` renders. Read only from `app/views`, it was
//! `themes/next/pages/index`, which no controller renders, and the action
//! answered 204.
//!
//! WHAT IS MODELED. Each class's list as ActionView 8.1's `PathRegistry`
//! keeps it: `ActionController::Base`, `ActionController::API` and
//! `ActionMailer::Base` each start from the app's own `app/views` and every
//! engine root after it (the order `app_roots` gives, as before); a class
//! with no list of its own reads its superclass's, so a later change to the
//! parent shows through; one that prepends or appends takes its own copy,
//! and a later change to the parent does not. Operations run in boot order
//! (`Phase`): the initializer files' statements, then their `to_prepare`
//! blocks, then the class bodies as eager loading reaches them, a superclass
//! before its subclasses. A class an initializer names is loaded at that
//! point, its body first, as Rails' autoloader does; an `on_load` hook runs
//! when its framework class first loads.
//!
//! The lookup itself becomes NAMES. Every template is ingested under the
//! name a render reaches it by — its path below the view path it is found
//! in — and an earlier view path's template shadows a later one's of the
//! same name and format, as engine roots already did. Every consumer
//! downstream (implicit renders, `render "x"`, partials, layouts, mailer
//! parts, ivar typing) looks templates up by name, so each gets Rails'
//! answer without knowing view paths exist. The template has one physical
//! file and keeps it: a template under a view path nested in another
//! (`app/views/themes/next` in `app/views`) is reachable under two names,
//! and only the inner one is kept unless something names the outer one
//! (`render "themes/next/x"`), or it is a controller's own directory.
//!
//! WHAT IS NOT. One name renders one template. Where two classes' lists find
//! different templates for one name, a template in a controller's or a
//! mailer's own directory is the one its owner finds, a layout or a template
//! only classes sharing one list render is theirs, and any other name keeps
//! the answer of `ApplicationController`'s list. A render whose own lookup
//! would find a different template than the one kept is an error naming
//! both, not a silent substitution. A path that is not a literal (a
//! resolver object, a computed string), a view path changed per request (in
//! an action, a filter or a `to_run` block), and a receiver that is neither
//! a framework base nor one of the app's controllers or mailers are errors
//! too.

use std::path::{Component, Path, PathBuf};

use ruby_prism::Node;

use crate::app::App;
use crate::det::{HashMap, HashSet};
use crate::dialect::{ControllerBodyItem, LayoutDecl, View};
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::span::Span;

const CONTROLLER_BASE: &str = "ActionController::Base";
const API_BASE: &str = "ActionController::API";
const MAILER_BASE: &str = "ActionMailer::Base";
const FRAMEWORK_BASES: &[&str] = &[CONTROLLER_BASE, API_BASE, MAILER_BASE];

/// One change to a class's view paths, in the order boot runs it.
#[derive(Clone, Debug)]
pub(super) struct ViewPathOp {
    receiver: String,
    /// `None` for a statement that only names the class, which loads it.
    change: Option<Change>,
    span: Span,
    /// When boot runs it: `Phase::Boot` for an initializer file's own
    /// statements, a later phase for a `to_prepare` block's.
    phase: Phase,
    /// Run in an `on_load` hook, once its class first loads.
    hook: bool,
    /// A `run_once: true` hook's identity: of the classes it hooks, only
    /// the first to load runs it (`ViewLookup::new` settles which).
    once: Option<(u32, u32)>,
}

/// Rails runs every initializer file, then the `to_prepare` blocks given
/// to the reloader itself (`ActiveSupport::Reloader.to_prepare`) as they
/// were registered, then those given to `config.to_prepare`, which the
/// application hands the reloader only once initializers have run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Boot,
    ReloaderPrepare,
    ConfigPrepare,
}

#[derive(Clone, Debug)]
enum Change {
    Prepend(Vec<PathBuf>),
    Append(Vec<PathBuf>),
    Replace(Vec<PathBuf>),
}

/// The view-path operations a `config/initializers` file runs, bare or in
/// a `to_prepare` block or an `ActiveSupport.on_load(:action_controller)`
/// (`:action_mailer`, …) block. A view-path call this cannot evaluate is
/// reported here, once.
pub(super) fn extract_initializer_view_paths(source: &[u8], file: &str) -> Vec<ViewPathOp> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(program) = root.as_program_node() else { return Vec::new() };
    let mut out = Vec::new();
    for top in program.statements().body().iter() {
        // `initializer_statements`' unwrapping, keeping which phase a
        // statement runs in.
        let phase = prepare_phase(&top);
        let to_run = is_to_run(&top);
        let stmts = match super::app::initializer_statements_of(&top) {
            Some(stmts) => stmts,
            None => vec![top],
        };
        for stmt in stmts {
            if !is_view_path_statement(&stmt) {
                if on_load_view_path_calls(&stmt).is_some_and(|(view_paths, all)| view_paths > 0 && view_paths < all) {
                    report(
                        super::util::node_span(&stmt, file),
                        "an `on_load` block that changes view paths among other statements is not read".to_string(),
                    );
                }
                // Any other statement naming a class loads it there
                // (`ApplicationController.include Guard` in a `to_prepare`):
                // its list is its own from then on, if its body makes one.
                let mut named = NamedConstants(Vec::new());
                if !to_run {
                    ruby_prism::Visit::visit(&mut named, &stmt);
                }
                for receiver in named.0 {
                    out.push(ViewPathOp { receiver, change: None, span: Span::synthetic(), phase, hook: false, once: None });
                }
                continue;
            }
            let Ok(expr) = super::expr::ingest_expr(&stmt, file) else { continue };
            if to_run {
                report(
                    expr.span,
                    "a `to_run` block changes the view paths around every request, which this build does not model"
                        .to_string(),
                );
                continue;
            }
            let from = out.len();
            initializer_ops(&expr, &mut out);
            for op in &mut out[from..] {
                op.phase = phase;
            }
        }
    }
    out
}

/// Whether an initializer statement is a view-path call, supported or not:
/// the initializer ledger's question, asked without reporting anything.
pub(super) fn reads_initializer_statement(text: &[u8]) -> bool {
    let result = ruby_prism::parse(text);
    let root = result.node();
    let Some(program) = root.as_program_node() else { return false };
    super::app::initializer_statements(&program).iter().any(is_view_path_statement)
}

/// The constants a statement names outside its blocks and method bodies,
/// which run later.
struct NamedConstants(Vec<String>);

impl<'pr> ruby_prism::Visit<'pr> for NamedConstants {
    fn visit_block_node(&mut self, _: &ruby_prism::BlockNode<'pr>) {}
    fn visit_lambda_node(&mut self, _: &ruby_prism::LambdaNode<'pr>) {}
    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        if let Some(receiver) = node.receiver() {
            self.visit(&receiver);
        }
    }
    fn visit_constant_read_node(&mut self, node: &ruby_prism::ConstantReadNode<'pr>) {
        self.0.push(super::util::constant_id_str(&node.name()).to_string());
    }
    fn visit_constant_path_node(&mut self, node: &ruby_prism::ConstantPathNode<'pr>) {
        if let Some(path) = super::util::constant_path_of(&node.as_node()) {
            self.0.push(path.join("::"));
        }
    }
}

fn is_to_run(stmt: &Node<'_>) -> bool {
    stmt.as_call_node().is_some_and(|c| super::util::constant_id_str(&c.name()) == "to_run")
}

fn prepare_phase(stmt: &Node<'_>) -> Phase {
    let Some(call) = stmt.as_call_node() else { return Phase::Boot };
    if super::util::constant_id_str(&call.name()) != "to_prepare" || call.block().is_none() {
        return Phase::Boot;
    }
    // Every railtie's `config` shares one `to_prepare` list, and
    // `Rails.configuration` is the application's `config`.
    let on_config = call.receiver().and_then(|r| r.as_call_node()).is_some_and(|r| {
        match super::util::constant_id_str(&r.name()) {
            "config" => true,
            "configuration" => r.receiver().and_then(|rr| super::util::constant_path_of(&rr)).is_some_and(|p| p == ["Rails"]),
            _ => false,
        }
    });
    if on_config { Phase::ConfigPrepare } else { Phase::ReloaderPrepare }
}

fn is_view_path_statement(node: &Node<'_>) -> bool {
    let Some(call) = node.as_call_node() else { return false };
    let name = super::util::constant_id_str(&call.name());
    if is_view_path_method(name) {
        return call.receiver().is_some_and(|r| super::util::constant_path_of(&r).is_some());
    }
    on_load_view_path_calls(node).is_some_and(|(view_paths, all)| view_paths == all)
}

/// For an `on_load` block of a hook with view paths: how many of its
/// statements are view-path calls, and how many it has.
fn on_load_view_path_calls(node: &Node<'_>) -> Option<(usize, usize)> {
    let call = node.as_call_node()?;
    if super::util::constant_id_str(&call.name()) != "on_load" || on_load_receivers_of(&call).is_empty() {
        return None;
    }
    let statements = call.block()?.as_block_node()?.body()?.as_statements_node()?;
    let all = statements.body().iter().count();
    let view_paths = statements
        .body()
        .iter()
        .filter(|stmt| {
            stmt.as_call_node().is_some_and(|c| {
                is_view_path_method(super::util::constant_id_str(&c.name()))
                    && c.receiver().is_none_or(|r| r.as_self_node().is_some())
            })
        })
        .count();
    Some((view_paths, all))
}

fn is_view_path_method(name: &str) -> bool {
    matches!(name, "prepend_view_path" | "append_view_path" | "view_paths=")
}

fn on_load_receivers_of(call: &ruby_prism::CallNode<'_>) -> &'static [&'static str] {
    if call.receiver().and_then(|r| super::util::constant_path_of(&r)).is_none_or(|p| p != ["ActiveSupport"]) {
        return &[];
    }
    let hook = call
        .arguments()
        .and_then(|a| a.arguments().iter().next())
        .and_then(|a| super::util::symbol_value(&a));
    on_load_receivers(hook.as_deref().unwrap_or(""))
}

/// The classes an `on_load` hook's block runs on: Rails runs
/// `:action_controller` for both controller bases.
fn on_load_receivers(hook: &str) -> &'static [&'static str] {
    match hook {
        "action_controller" => &[CONTROLLER_BASE, API_BASE],
        "action_controller_base" => &[CONTROLLER_BASE],
        "action_controller_api" => &[API_BASE],
        "action_mailer" => &[MAILER_BASE],
        _ => &[],
    }
}

fn initializer_ops(expr: &Expr, out: &mut Vec<ViewPathOp>) {
    if let ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. } = &*expr.node
        && method.as_str() == "on_load"
        && const_name(recv).as_deref() == Some("ActiveSupport")
    {
        // `yield:` changes nothing here; `run_once:` runs the block for the
        // first of the hooked classes to load only.
        let once = args.iter().skip(1).any(|a| {
            matches!(&*a.node, ExprNode::Hash { entries, .. } if entries.iter().any(|(k, v)| {
                matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "run_once")
                    && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } })
            }))
        });
        let once = once.then_some((expr.span.file.0, expr.span.start));
        let hook = match args.as_slice() {
            [arg, rest @ ..] if rest.iter().all(|r| matches!(&*r.node, ExprNode::Hash { .. })) => match &*arg.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        if on_load_receivers(&hook).is_empty() {
            report(expr.span, "an `on_load` hook whose arguments are not a hook name and options".to_string());
            return;
        }
        let ExprNode::Lambda { body, .. } = &*block.node else { return };
        for stmt in statements(body) {
            let Some((recv, change)) = view_path_call(stmt) else { continue };
            if recv.is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef)) {
                report(stmt.span, "a view-path call on another receiver inside `on_load`".to_string());
                continue;
            }
            match change {
                Ok(change) => {
                    for receiver in on_load_receivers(&hook) {
                        out.push(ViewPathOp {
                            receiver: receiver.to_string(),
                            change: Some(change.clone()),
                            span: stmt.span,
                            phase: Phase::Boot,
                            hook: true,
                            once,
                        });
                    }
                }
                Err(reason) => report(stmt.span, reason),
            }
        }
        return;
    }
    let Some((recv, change)) = view_path_call(expr) else { return };
    let Some(receiver) = recv.and_then(const_name) else {
        report(expr.span, "a view-path call whose receiver is not a constant".to_string());
        return;
    };
    match change {
        Ok(change) => out.push(ViewPathOp { receiver, change: Some(change), span: expr.span, phase: Phase::Boot, hook: false, once: None }),
        Err(reason) => report(expr.span, reason),
    }
}

/// The view-path operations of a controller's or mailer's class body, in
/// source order. Called once per class.
fn class_body_ops<'a>(class: &str, exprs: impl IntoIterator<Item = &'a Expr>) -> Vec<ViewPathOp> {
    let mut out = Vec::new();
    for expr in exprs {
        let Some((recv, change)) = view_path_call(expr) else { continue };
        if recv.is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef)) {
            continue;
        }
        match change {
            Ok(change) => {
                out.push(ViewPathOp { receiver: class.to_string(), change: Some(change), span: expr.span, phase: Phase::Boot, hook: false, once: None })
            }
            Err(reason) => report(expr.span, reason),
        }
    }
    out
}

/// A view-path change inside a method or a filter block runs per request,
/// on the lookup context of that one request: not modeled.
fn report_per_request_changes(owner: &str, site: &str, body: &Expr) {
    fn walk(e: &Expr, owner: &str, site: &str) {
        if let Some((recv, _)) = view_path_call(e)
            && recv.is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
        {
            report(
                e.span,
                format!("{site} changes the view paths of one `{owner}` request, which this build does not model"),
            );
        }
        e.node.for_each_child(&mut |c| walk(c, owner, site));
    }
    walk(body, owner, site);
}

/// `recv.prepend_view_path(x)` / `recv.append_view_path(x)` /
/// `recv.view_paths = x`: the receiver (if written) and the change, or why
/// the argument cannot be read.
fn view_path_call(expr: &Expr) -> Option<(Option<&Expr>, Result<Change, String>)> {
    match &*expr.node {
        // A block is accepted and ignored, as Ruby does for a method that
        // takes none.
        ExprNode::Send { recv, method, args, .. } => {
            let change: fn(Vec<PathBuf>) -> Change = match method.as_str() {
                "prepend_view_path" => Change::Prepend,
                "append_view_path" => Change::Append,
                "view_paths=" => Change::Replace,
                _ => return None,
            };
            let paths = match args.as_slice() {
                [arg] => paths_of(arg),
                _ => Err(format!("`{}` takes one argument here", method.as_str())),
            };
            Some((recv.as_ref(), paths.map(change)))
        }
        ExprNode::Assign { target: LValue::Attr { recv, name }, value }
            if matches!(name.as_str(), "view_paths" | "view_paths=") =>
        {
            Some((Some(recv), paths_of(value).map(Change::Replace)))
        }
        _ => None,
    }
}

fn paths_of(arg: &Expr) -> Result<Vec<PathBuf>, String> {
    match &*arg.node {
        ExprNode::Array { elements, .. } => elements.iter().map(path_of).collect(),
        _ => Ok(vec![path_of(arg)?]),
    }
}

/// A view path the build can name: a string literal relative to the app
/// (Rails expands it against `Rails.root`, its working directory),
/// `Rails.root.join("a", "b")`, or `"#{Rails.root}/a/b"`, optionally `.to_s`.
fn path_of(e: &Expr) -> Result<PathBuf, String> {
    let unreadable = || {
        Err("a view path that is not a string literal or `Rails.root.join(...)` of literals (a resolver, a computed path)".to_string())
    };
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => normalize(value),
        ExprNode::Send { recv: Some(recv), method, args, .. } if method.as_str() == "to_s" && args.is_empty() => {
            path_of(recv)
        }
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "join" && is_rails_root(recv) =>
        {
            let mut parts = Vec::new();
            for arg in args {
                let ExprNode::Lit { value: Literal::Str { value } } = &*arg.node else { return unreadable() };
                // `Pathname#join` restarts at an absolute part.
                if value.starts_with('/') {
                    return normalize(value);
                }
                parts.push(value.as_str());
            }
            normalize(&parts.join("/"))
        }
        _ if is_rails_root(e) => normalize(""),
        ExprNode::StringInterp { parts } => match parts.as_slice() {
            [InterpPart::Expr { expr }, InterpPart::Text { value }] if is_rails_root(expr) && value.starts_with('/') => {
                normalize(value.trim_start_matches('/'))
            }
            _ => unreadable(),
        },
        _ => unreadable(),
    }
}

fn is_rails_root(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { recv: Some(r), method, args, .. }
        if method.as_str() == "root" && args.is_empty() && const_name(r).as_deref() == Some("Rails"))
}

/// The path below the app's root, `.` and `..` folded.
fn normalize(path: &str) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir if out.pop() => {}
            _ => return Err(format!("the view path `{path}` is outside the app")),
        }
    }
    Ok(out)
}

fn const_name(e: &Expr) -> Option<String> {
    let ExprNode::Const { path } = &*e.node else { return None };
    Some(path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::"))
}

fn statements(body: &Expr) -> Vec<&Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    }
}

fn report(span: Span, detail: String) {
    super::prism::push(Diagnostic::unsupported(span, None, "view_paths", detail));
}

/// A controller or a mailer: a class whose templates live in its own
/// directory and which looks them up through its view paths.
struct Member {
    name: String,
    /// `Admin::PagesController` → `admin/pages`, `UserMailer` → `user_mailer`.
    dir: String,
    context: usize,
    /// The layouts its renders are wrapped in, by name.
    layouts: Vec<String>,
    /// The templates its own bodies render by name, with the span.
    renders: Vec<(String, Span)>,
    /// The templates its actions, own and inherited, render by a name
    /// without a directory (an action that can end in an implicit render,
    /// `render :show`, `render partial: "form"`), and its ancestors'
    /// directories, nearest first: a name its own directory lacks renders
    /// the first ancestor's, as Rails' prefixes do.
    relative: Vec<(String, Span)>,
    prefixes: Vec<String>,
    /// A scoped `layout false` (`only:` / `except:`) up its chain, and the
    /// renders (by span) of the actions it leaves without a layout.
    layout_scope: Option<LayoutDecl>,
    layout_free: HashSet<Span>,
}

/// Every class's view paths and the distinct lists among them.
pub(super) struct ViewLookup {
    /// Every directory some list searches, the app's and engines' first.
    roots: Vec<PathBuf>,
    /// Each distinct list as indices into `roots`.
    contexts: Vec<Vec<usize>>,
    members: Vec<Member>,
    /// The list a name no member owns is kept from: `ApplicationController`'s.
    primary: usize,
    /// Each render call's own `layout: false` and `formats:`, by its span.
    options: HashMap<Span, RenderOptions>,
}

#[derive(Default)]
struct RenderOptions {
    without_layout: bool,
    formats: Option<Vec<String>>,
}

/// One searched directory as ingest walked it: its templates, in walk order.
pub(super) struct WalkedRoot {
    pub(super) root: PathBuf,
    pub(super) dir: PathBuf,
    pub(super) files: Vec<PathBuf>,
}

impl ViewLookup {
    pub(super) fn new(app: &App, base: Vec<PathBuf>, initializer_ops: &[ViewPathOp]) -> Self {
        let mailers = crate::lower::mailer_class_side::mailer_names(app);
        let mut classes: Vec<Class> = Vec::new();
        for c in &app.controllers {
            let unknown = c.body.iter().filter_map(|item| match item {
                ControllerBodyItem::Unknown { expr, .. } => Some(expr),
                _ => None,
            });
            let name = c.name.0.as_str();
            for action in c.actions() {
                report_per_request_changes(name, &format!("`{name}#{}`", action.name.as_str()), &action.body);
            }
            // A filter block (`before_action { prepend_view_path … }`).
            for expr in unknown.clone().filter(|e| view_path_call(e).is_none()) {
                report_per_request_changes(name, "a class-body block", expr);
            }
            classes.push(Class {
                name: name.to_string(),
                parent: c.parent.as_ref().map(|p| written_parent(p.0.as_str(), c.parent_span)),
                mailer: false,
                ops: class_body_ops(name, unknown),
            });
        }
        for lc in app.library_classes.iter().filter(|lc| mailers.contains(lc.name.0.as_str())) {
            let name = lc.name.0.as_str();
            for m in &lc.methods {
                report_per_request_changes(name, &format!("`{name}#{}`", m.name.as_str()), &m.body);
            }
            classes.push(Class {
                name: name.to_string(),
                parent: lc.parent.as_ref().map(|p| written_parent(p.0.as_str(), lc.parent_span)),
                mailer: true,
                ops: class_body_ops(name, &lc.unknown_calls),
            });
        }
        classes.sort_by(|a, b| a.name.cmp(&b.name));
        classes.dedup_by(|a, b| a.name == b.name);

        let mut registry = Registry::new(&classes, base.clone());
        // Every initializer file's statements, then the `to_prepare` blocks
        // (`Phase`); an `on_load` hook waits for its class to load, which
        // the first statement naming it or a subclass does, and eager
        // loading does last.
        // A `run_once` hook on both controller bases runs for whichever
        // loads first, which boot does not settle when the app has
        // controllers under each; with controllers under one, it is that one.
        let mut skipped: HashSet<(u32, u32, String)> = HashSet::default();
        let mut hooks: Vec<(u32, u32)> = initializer_ops.iter().filter_map(|op| op.once).collect();
        hooks.sort();
        hooks.dedup();
        for id in hooks {
            let group: Vec<&ViewPathOp> = initializer_ops.iter().filter(|o| o.once == Some(id)).collect();
            let mut receivers: Vec<&str> = Vec::new();
            for o in &group {
                if !receivers.contains(&o.receiver.as_str()) {
                    receivers.push(&o.receiver);
                }
            }
            let used: Vec<&str> = receivers
                .iter()
                .copied()
                .filter(|r| classes.iter().any(|c| registry.descends_from(&c.name, r)))
                .collect();
            let runs = match used.as_slice() {
                [] => Some(receivers[0]),
                [one] => Some(*one),
                _ => {
                    report(
                        group[0].span,
                        "a `run_once` hook on controller bases the app uses both of: which loads first is not settled at boot".to_string(),
                    );
                    None
                }
            };
            for receiver in receivers.into_iter().filter(|r| Some(*r) != runs) {
                skipped.insert((id.0, id.1, receiver.to_string()));
            }
        }
        let mut ordered: Vec<&ViewPathOp> = initializer_ops
            .iter()
            .filter(|op| op.once.is_none_or(|id| !skipped.contains(&(id.0, id.1, op.receiver.clone()))))
            .collect();
        ordered.sort_by_key(|op| op.phase);
        for op in ordered {
            if op.hook {
                registry.hook(op);
                continue;
            }
            if registry.index.contains_key(op.receiver.as_str()) {
                registry.load(&op.receiver);
            } else if FRAMEWORK_BASES.contains(&op.receiver.as_str()) {
                registry.reference(&op.receiver);
            } else if op.change.is_none() {
                continue;
            } else {
                report(
                    op.span,
                    format!(
                        "view paths changed on `{}`, which is neither a framework base nor one of the app's controllers or mailers",
                        op.receiver
                    ),
                );
                continue;
            }
            if let Some(change) = &op.change {
                registry.apply(&op.receiver, change);
            }
        }
        for framework in FRAMEWORK_BASES {
            registry.reference(framework);
        }
        for class in &classes {
            registry.load(&class.name);
        }
        // `class Admin::PagesController < BaseController` names the
        // top-level class, `module Admin; class PagesController <
        // BaseController` the namespaced one: ingest cannot tell the two
        // spellings apart yet, so a choice that changes the lookup is
        // reported.
        for (i, class) in classes.iter().enumerate() {
            let (Some(near), Some(written)) = (registry.parent(i), class.parent.as_ref()) else { continue };
            if near != *written && registry.index.contains_key(written.as_str()) && registry.get(&near) != registry.get(written) {
                report(
                    Span::synthetic(),
                    format!(
                        "`{}`'s superclass `{written}` may be `{near}` or `::{written}`, whose view paths differ",
                        class.name
                    ),
                );
            }
        }

        let mut roots: Vec<PathBuf> = base.clone();
        let mut contexts: Vec<Vec<usize>> = Vec::new();
        let mut members = Vec::new();
        for class in &classes {
            let mut list: Vec<usize> = Vec::new();
            for path in registry.get(&class.name) {
                let index = match roots.iter().position(|r| *r == path) {
                    Some(i) => i,
                    None => {
                        roots.push(path);
                        roots.len() - 1
                    }
                };
                if !list.contains(&index) {
                    list.push(index);
                }
            }
            let context = match contexts.iter().position(|c| *c == list) {
                Some(i) => i,
                None => {
                    contexts.push(list);
                    contexts.len() - 1
                }
            };
            let (mut layouts, renders, layout_scope, mut layout_free) = if class.mailer {
                (mailer_layouts(app, &class.name), Vec::new(), None, HashSet::default())
            } else {
                controller_references(app, &class.name)
            };
            // `ActionController::API` renders no layout.
            if registry.descends_from(&class.name, API_BASE) {
                layouts.clear();
            }
            // A mailer action renders `<mailer>/<action>`.
            let (relative, prefixes) = if class.mailer {
                let methods = app.library_classes.iter().filter(|lc| lc.name.0.as_str() == class.name).flat_map(|lc| &lc.methods);
                let actions = methods
                    .filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance && m.name.as_str() != "initialize")
                    .map(|m| (crate::ingest::mailer_callbacks::action_of(m.name.as_str()).to_string(), m.body.span))
                    .collect();
                (actions, Vec::new())
            } else {
                relative_renders(app, &class.name, layout_scope.as_ref(), &mut layout_free)
            };
            members.push(Member {
                dir: view_dir(&class.name, class.mailer),
                name: class.name.clone(),
                context,
                layouts,
                renders,
                relative,
                prefixes,
                layout_scope,
                layout_free,
            });
        }
        let mut class_renders = Vec::new();
        for c in &app.controllers {
            for action in c.actions() {
                collect_class_renders(&action.body, &mut class_renders);
            }
        }
        for m in app.models.iter().flat_map(|m| m.methods()).chain(app.library_classes.iter().flat_map(|lc| &lc.methods)) {
            collect_class_renders(&m.body, &mut class_renders);
        }
        for (class, render) in class_renders {
            if let Some(member) = members.iter_mut().find(|m| m.name == class) {
                member.renders.push(render);
            }
        }
        if contexts.is_empty() {
            contexts.push((0..base.len()).collect());
        }
        let primary = members.iter().find(|m| m.name == "ApplicationController").map_or(0, |m| m.context);
        let mut options = HashMap::default();
        let bodies = app
            .controllers
            .iter()
            .flat_map(|c| c.actions().map(|a| &a.body))
            .chain(app.library_classes.iter().flat_map(|lc| lc.methods.iter().map(|m| &m.body)))
            .chain(app.models.iter().flat_map(|m| m.methods().map(|m| &m.body)));
        for body in bodies {
            collect_render_options(body, &mut options);
        }
        ViewLookup { roots, contexts, members, primary, options }
    }

    /// The directories to walk, in order.
    pub(super) fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The templates the app's renders reach, each under the name it is
    /// reached by, in walk order. `ingested` holds each walked file's
    /// template as ingested under the first directory that reached it.
    pub(super) fn select(
        &self,
        app: &App,
        app_dir: &Path,
        walked: &[WalkedRoot],
        ingested: &HashMap<PathBuf, View>,
    ) -> Vec<View> {
        // Every (root, file) with the name and format it answers to.
        let mut candidates: Vec<Candidate> = Vec::new();
        for walk in walked {
            let Some(root) = self.roots.iter().position(|r| *r == walk.root) else { continue };
            for file in &walk.files {
                let Some(view) = ingested.get(file) else { continue };
                let Some(rel) = file.strip_prefix(&walk.dir).ok() else { continue };
                let base = view.name.as_str().rsplit('/').next().unwrap_or("");
                let name = match rel.parent().map(|p| p.to_string_lossy().replace('\\', "/")) {
                    Some(dir) if !dir.is_empty() => format!("{dir}/{base}"),
                    _ => base.to_string(),
                };
                candidates.push(Candidate { root, file: file.clone(), name, format: view.format.as_str().to_string() });
            }
        }
        // Each list's answer for each name: its first root's template.
        let spaces: Vec<HashMap<(&str, &str), usize>> = self
            .contexts
            .iter()
            .map(|context| {
                let mut space: HashMap<(&str, &str), usize> = HashMap::default();
                for &root in context {
                    for (i, c) in candidates.iter().enumerate().filter(|(_, c)| c.root == root) {
                        space.entry((c.name.as_str(), c.format.as_str())).or_insert(i);
                    }
                }
                space
            })
            .collect();
        let mut keys: Vec<(&str, &str)> = spaces.iter().flat_map(|s| s.keys().copied()).collect();
        keys.sort();
        keys.dedup();

        let mut chosen: HashMap<(&str, &str), usize> = HashMap::default();
        let mut contested: Vec<(&str, &str)> = Vec::new();
        for key in keys {
            let answers: Vec<Option<&PathBuf>> =
                spaces.iter().map(|s| s.get(&key).map(|&i| &candidates[i].file)).collect();
            let pick = if answers.iter().all(|a| *a == answers[0]) {
                spaces.iter().find_map(|s| s.get(&key).copied())
            } else {
                contested.push(key);
                match self.owner(key.0).or_else(|| self.sole_user(key.0)) {
                    Some(context) => spaces[context].get(&key).copied(),
                    None => spaces[self.primary]
                        .get(&key)
                        .or_else(|| spaces.iter().find_map(|s| s.get(&key)))
                        .copied(),
                }
            };
            if let Some(i) = pick {
                chosen.insert(key, i);
            }
        }

        // A file reached under two names, through nested view paths, is
        // kept under its innermost one; an outer name stays only when
        // something renders it.
        let mut names_of: HashMap<&PathBuf, Vec<usize>> = HashMap::default();
        for &i in chosen.values() {
            names_of.entry(&candidates[i].file).or_default().push(i);
        }
        let literals = std::cell::OnceCell::new();
        let mut kept: HashSet<usize> = HashSet::default();
        let mut pruned = false;
        for picks in names_of.values() {
            let depth = |i: usize| self.roots[candidates[i].root].components().count();
            let deepest = picks.iter().map(|&i| depth(i)).max().unwrap_or(0);
            for &i in picks {
                let name = candidates[i].name.as_str();
                if depth(i) == deepest
                    || self.owner(name).is_some()
                    || literals.get_or_init(|| RenderLiterals::collect(app, ingested)).names(name)
                {
                    kept.insert(i);
                } else {
                    pruned = true;
                }
            }
        }
        // A kept template's own renders stay reachable: a bare `render
        // "item"` in `themes/next/nav/_menu` names `themes/next/nav/_item`,
        // which no string spells out.
        if pruned {
            let mut by_name: HashMap<&str, Vec<usize>> = HashMap::default();
            for (key, &i) in &chosen {
                by_name.entry(key.0).or_default().push(i);
            }
            let mut work: Vec<usize> = kept.iter().copied().collect();
            while let Some(i) = work.pop() {
                let c = &candidates[i];
                let dir = c.name.rsplit_once('/').map_or("", |(d, _)| d);
                for (name, _) in template_renders(&ingested[&c.file].body, dir) {
                    for &j in by_name.get(name.as_str()).into_iter().flatten() {
                        if kept.insert(j) {
                            work.push(j);
                        }
                    }
                }
            }
        }

        if !contested.is_empty() {
            self.check(&candidates, &spaces, &chosen, &contested, &kept, ingested, app_dir);
        }

        let mut out = Vec::new();
        for (i, c) in candidates.iter().enumerate() {
            if !kept.contains(&i) {
                continue;
            }
            let mut view = ingested[&c.file].clone();
            if view.name.as_str() != c.name {
                view.name = crate::ident::Symbol::from(c.name.as_str());
            }
            out.push(view);
        }
        out
    }

    /// The list a template in a member's own directory is found through.
    fn owner(&self, name: &str) -> Option<usize> {
        let (dir, _) = name.rsplit_once('/')?;
        if dir == "layouts" || dir.starts_with("layouts/") {
            return None;
        }
        self.members.iter().find(|m| m.dir == dir).map(|m| m.context)
    }

    /// The list of every member that renders `name` itself (a layout, a
    /// `render "x/y"`), when they share one: `layouts/mailer` is the
    /// mailers' even when the controllers' list finds another.
    fn sole_user(&self, name: &str) -> Option<usize> {
        let mut users = self
            .members
            .iter()
            .filter(|m| m.layouts.iter().any(|l| l == name) || m.renders.iter().any(|(r, _)| r == name))
            .map(|m| m.context);
        let first = users.next()?;
        users.all(|c| c == first).then_some(first)
    }

    /// Report each render whose own lookup finds a different template than
    /// the one kept under the name it renders.
    fn check(
        &self,
        candidates: &[Candidate],
        spaces: &[HashMap<(&str, &str), usize>],
        chosen: &HashMap<(&str, &str), usize>,
        contested: &[(&str, &str)],
        kept: &HashSet<usize>,
        ingested: &HashMap<PathBuf, View>,
        app_dir: &Path,
    ) {
        let file_of = |i: Option<usize>| i.map(|i| candidates[i].file.clone());
        let shown = |f: &Option<PathBuf>| match f {
            Some(f) => format!("`{}`", f.strip_prefix(app_dir).unwrap_or(f).display()),
            None => "no template".to_string(),
        };
        let mut formats: HashMap<&str, Vec<&str>> = HashMap::default();
        for &(n, format) in contested {
            formats.entry(n).or_default().push(format);
        }
        let mut seen: HashSet<(String, String, usize)> = HashSet::default();
        // A member's list is known; a template's are those that can reach
        // it, which is wider than those that do, so a template is judged
        // only where its lookup finds another file, not where it finds none.
        let mut judge = |site: &str, span: Span, name: &str, contexts: &[(usize, bool)], rendered: &dyn Fn(&str) -> bool| {
            let Some((n, formats)) = formats.get_key_value(name) else { return };
            for &format in formats.iter().filter(|f| rendered(f)) {
                let n = *n;
                let kept_file = file_of(chosen.get(&(n, format)).copied());
                for &(context, exact) in contexts {
                    let theirs = file_of(spaces[context].get(&(n, format)).copied());
                    if theirs == kept_file
                        || (theirs.is_none() && !exact)
                        || !seen.insert((site.to_string(), format!("{n}.{format}"), context))
                    {
                        continue;
                    }
                    report(
                        span,
                        format!(
                            "{site} renders `{n}` ({format}), which its view paths find at {} but this build renders from {}: a name renders one template here, not one per lookup",
                            shown(&theirs),
                            shown(&kept_file)
                        ),
                    );
                }
            }
        };
        // Which lists each kept template is rendered through: its owner's,
        // each member's that renders it by name or as a layout, and, along
        // every render, its renderer's. A template no member reaches that
        // way is judged under each list that can find it, loosely.
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::default();
        for &i in kept {
            by_name.entry(candidates[i].name.as_str()).or_default().push(i);
        }
        let mut renders: HashMap<usize, Vec<(String, Span)>> = HashMap::default();
        for &i in kept {
            let c = &candidates[i];
            let dir = c.name.rsplit_once('/').map_or("", |(d, _)| d);
            renders.insert(i, template_renders(&ingested[&c.file].body, dir));
        }
        let mut reached: HashMap<usize, std::collections::BTreeSet<usize>> = HashMap::default();
        let mut work: Vec<(usize, usize)> = Vec::new();
        for &i in kept {
            if let Some(context) = self.owner(&candidates[i].name) {
                work.push((i, context));
            }
        }
        for member in &self.members {
            let site = format!("`{}`", member.name);
            let space = &spaces[member.context];
            let has = |name: &str| space.keys().any(|(n, _)| *n == name);
            // What a relative name renders through an ancestor's prefix,
            // its own directory having no such template.
            let inherited: Vec<(String, Span)> = member
                .relative
                .iter()
                .filter(|(target, _)| !has(&format!("{}/{target}", member.dir)))
                .filter_map(|(target, span)| {
                    member.prefixes.iter().map(|p| format!("{p}/{target}")).find(|n| has(n)).map(|n| (n, *span))
                })
                .collect();
            let is_template = |n: &str| !n.starts_with("layouts/") && n.rsplit_once('/').is_some_and(|(_, base)| !base.starts_with('_'));
            // Its layout wraps what it renders as a page: a template of its
            // own directory, one it names, or one an ancestor's prefix finds.
            // A render's own `layout: false` / `formats:`, and the actions a
            // scoped `layout false` covers.
            let with_layout = |span: &Span| {
                !member.layout_free.contains(span) && !self.options.get(span).is_some_and(|o| o.without_layout)
            };
            let in_formats = |span: &Span, format: &str| {
                self.options.get(span).and_then(|o| o.formats.as_ref()).is_none_or(|fs| fs.iter().any(|f| f == format))
            };
            let own: Vec<(String, Span)> = member
                .relative
                .iter()
                .map(|(target, span)| (format!("{}/{target}", member.dir), *span))
                .filter(|(name, _)| has(name))
                .collect();
            let pages: Vec<&(String, Span)> = own
                .iter()
                .chain(&member.renders)
                .chain(&inherited)
                .filter(|(n, span)| is_template(n) && with_layout(span))
                .collect();
            let layouts: &[String] = if pages.is_empty() { &[] } else { &member.layouts };
            // A layout is looked up in the formats the member renders pages in.
            let own_formats: HashSet<&str> = space
                .keys()
                .filter(|(n, f)| pages.iter().any(|(r, span)| r == n && in_formats(span, f)))
                .map(|(_, f)| *f)
                .collect();
            let in_own = |f: &str| own_formats.contains(f);
            for layout in layouts {
                judge(&site, Span::synthetic(), layout, &[(member.context, true)], &in_own);
            }
            for (name, span) in member.renders.iter().chain(&inherited) {
                judge(&site, *span, name, &[(member.context, true)], &|f| in_formats(span, f));
            }
            for layout in layouts {
                for &j in by_name.get(layout.as_str()).into_iter().flatten().filter(|&&j| in_own(&candidates[j].format)) {
                    work.push((j, member.context));
                }
            }
            for (name, _) in member.renders.iter().chain(&inherited) {
                for &j in by_name.get(name.as_str()).into_iter().flatten() {
                    work.push((j, member.context));
                }
            }
        }
        while let Some((i, context)) = work.pop() {
            if !reached.entry(i).or_default().insert(context) {
                continue;
            }
            let format = candidates[i].format.as_str();
            for (name, _) in &renders[&i] {
                for &j in by_name.get(name.as_str()).into_iter().flatten().filter(|&&j| renders_in(format, &candidates[j].format)) {
                    work.push((j, context));
                }
            }
        }
        for &i in kept {
            let c = &candidates[i];
            let contexts: Vec<(usize, bool)> = match reached.get(&i) {
                Some(contexts) => contexts.iter().map(|&k| (k, true)).collect(),
                None => (0..spaces.len())
                    .filter(|&k| spaces[k].get(&(c.name.as_str(), c.format.as_str())).is_some_and(|&j| candidates[j].file == c.file))
                    .map(|k| (k, false))
                    .collect(),
            };
            let site = format!("template `{}`", c.name);
            for (name, span) in &renders[&i] {
                judge(&site, *span, name, &contexts, &|f| renders_in(&c.format, f));
            }
        }
    }
}

/// Whether a template of `format` renders partials of `partial`: its own
/// format's, and HTML ones too unless it is plain text.
fn renders_in(format: &str, partial: &str) -> bool {
    partial == format || (partial == "html" && format != "text")
}

struct Candidate {
    root: usize,
    file: PathBuf,
    name: String,
    format: String,
}

struct Class {
    name: String,
    parent: Option<String>,
    mailer: bool,
    ops: Vec<ViewPathOp>,
}

/// `ActionView::PathRegistry`: a class's own list, or its superclass's.
struct Registry<'a> {
    classes: &'a [Class],
    index: HashMap<&'a str, usize>,
    own: HashMap<String, Vec<PathBuf>>,
    loaded: HashSet<usize>,
    /// The framework bases loaded so far, and the hooks waiting for one.
    referenced: HashSet<&'static str>,
    hooks: Vec<(String, Change)>,
}

impl<'a> Registry<'a> {
    fn new(classes: &'a [Class], base: Vec<PathBuf>) -> Self {
        let index = classes.iter().enumerate().map(|(i, c)| (c.name.as_str(), i)).collect();
        let mut own: HashMap<String, Vec<PathBuf>> = HashMap::default();
        for framework in [CONTROLLER_BASE, API_BASE, MAILER_BASE] {
            own.insert(framework.to_string(), base.clone());
        }
        Registry { classes, index, own, loaded: HashSet::default(), referenced: HashSet::default(), hooks: Vec::new() }
    }

    /// The superclass as Ruby resolves the name from inside the class's
    /// namespace: `Admin::PagesController < BaseController` is
    /// `Admin::BaseController` when the app has one.
    fn parent(&self, class: usize) -> Option<String> {
        let c = &self.classes[class];
        let parent = c.parent.as_ref()?;
        if let Some(absolute) = parent.strip_prefix("::") {
            return Some(absolute.to_string());
        }
        let mut scope: Vec<&str> = c.name.split("::").collect();
        scope.pop();
        while !scope.is_empty() {
            let candidate = format!("{}::{parent}", scope.join("::"));
            if self.index.contains_key(candidate.as_str()) {
                return Some(candidate);
            }
            scope.pop();
        }
        Some(parent.clone())
    }

    fn get(&self, class: &str) -> Vec<PathBuf> {
        let mut current = class.to_string();
        for _ in 0..64 {
            if let Some(list) = self.own.get(&current) {
                return list.clone();
            }
            let Some(&i) = self.index.get(current.as_str()) else { break };
            match self.parent(i) {
                Some(parent) => current = parent,
                None => break,
            }
        }
        // A chain that leaves the app through a gem's class: that class
        // descends from the framework base of the same kind.
        let mailer = self.index.get(class).is_some_and(|&i| self.classes[i].mailer);
        self.own[if mailer { MAILER_BASE } else { CONTROLLER_BASE }].clone()
    }

    fn descends_from(&self, class: &str, framework: &str) -> bool {
        let mut current = class.to_string();
        for _ in 0..64 {
            if current == framework {
                return true;
            }
            let Some(&i) = self.index.get(current.as_str()) else { return false };
            let Some(parent) = self.parent(i) else { return false };
            current = parent;
        }
        false
    }

    /// An `on_load` hook: run now when its class has loaded, else when it does.
    fn hook(&mut self, op: &ViewPathOp) {
        let Some(change) = &op.change else { return };
        if self.referenced.contains(op.receiver.as_str()) {
            self.apply(&op.receiver, change);
        } else {
            self.hooks.push((op.receiver.clone(), change.clone()));
        }
    }

    /// A framework base loads: its waiting hooks run, in registration order.
    fn reference(&mut self, framework: &str) {
        let Some(&framework) = FRAMEWORK_BASES.iter().find(|f| **f == framework) else { return };
        if !self.referenced.insert(framework) {
            return;
        }
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.hooks).into_iter().partition(|(r, _)| r == framework);
        self.hooks = later;
        for (receiver, change) in now {
            self.apply(&receiver, &change);
        }
    }

    fn load(&mut self, class: &str) {
        let Some(&i) = self.index.get(class) else { return };
        if !self.loaded.insert(i) {
            return;
        }
        match self.parent(i) {
            Some(parent) if self.index.contains_key(parent.as_str()) => self.load(&parent),
            Some(parent) => self.reference(&parent),
            None => {}
        }
        for change in self.classes[i].ops.iter().filter_map(|op| op.change.as_ref()) {
            self.apply(class, change);
        }
    }

    fn apply(&mut self, class: &str, change: &Change) {
        let current = self.get(class);
        let list = match change {
            Change::Prepend(paths) => paths.iter().cloned().chain(current).collect(),
            Change::Append(paths) => current.into_iter().chain(paths.iter().cloned()).collect(),
            Change::Replace(paths) => paths.clone(),
        };
        self.own.insert(class.to_string(), list);
    }
}

/// The superclass as written, `::` kept: `< ::BaseController` names the
/// top-level class from any namespace.
fn written_parent(parent: &str, span: Span) -> String {
    let rooted = super::sources::path_of(span.file)
        .and_then(|path| super::sources::with_text(&path, |text| text.get(span.start as usize..).is_some_and(|t| t.starts_with("::"))))
        .unwrap_or(false);
    if rooted && !parent.starts_with("::") { format!("::{parent}") } else { parent.to_string() }
}

fn view_dir(class: &str, mailer: bool) -> String {
    let stem = if mailer { class } else { class.strip_suffix("Controller").unwrap_or(class) };
    stem.split("::").map(crate::naming::snake_case).collect::<Vec<_>>().join("/")
}

/// A controller's layouts (the nearest `layout "x"` up its chain, else
/// `layouts/application`) and the templates its methods render by name.
fn controller_references(app: &App, class: &str) -> (Vec<String>, Vec<(String, Span)>, Option<LayoutDecl>, HashSet<Span>) {
    let by_name: HashMap<&str, &crate::dialect::Controller> =
        app.controllers.iter().map(|c| (c.name.0.as_str(), c)).collect();
    let mut layouts = vec!["layouts/application".to_string()];
    let mut scope = None;
    let mut current = by_name.get(class).copied();
    for _ in 0..64 {
        let Some(c) = current else { break };
        match &c.layout {
            LayoutDecl::Name { name } => {
                layouts = vec![format!("layouts/{}", name.as_str())];
                break;
            }
            LayoutDecl::None { only, except } if only.is_empty() && except.is_empty() => {
                layouts.clear();
                break;
            }
            // `layout false, only: :index`: those actions go without it.
            decl @ LayoutDecl::None { .. } if scope.is_none() => scope = Some(decl.clone()),
            _ => {}
        }
        current = c.parent.as_ref().and_then(|p| by_name.get(p.0.as_str().trim_start_matches("::")).copied());
    }
    // Its own methods and those it inherits: an inherited action renders
    // through the subclass's lookup.
    let mut renders = Vec::new();
    let mut layout_free: HashSet<Span> = HashSet::default();
    let mut defined: HashSet<&str> = HashSet::default();
    let mut current = by_name.get(class).copied();
    for _ in 0..64 {
        let Some(c) = current else { break };
        for action in c.actions() {
            if defined.insert(action.name.as_str()) {
                let from = renders.len();
                collect_controller_renders(&action.body, &mut renders);
                if scope.as_ref().is_some_and(|s: &LayoutDecl| s.suppresses(&action.name)) {
                    layout_free.extend(renders[from..].iter().map(|(_, span)| *span));
                    layout_free.insert(action.body.span);
                }
            }
        }
        for m in included_methods(app, c) {
            collect_controller_renders(&m.body, &mut renders);
        }
        current = c.parent.as_ref().and_then(|p| by_name.get(p.0.as_str().trim_start_matches("::")).copied());
    }
    (layouts, renders, scope, layout_free)
}

/// A controller's relative render targets, own and inherited (see
/// `Member::relative`), and its app ancestors' template directories,
/// nearest first.
fn relative_renders(
    app: &App,
    class: &str,
    scope: Option<&LayoutDecl>,
    layout_free: &mut HashSet<Span>,
) -> (Vec<(String, Span)>, Vec<String>) {
    let by_name: HashMap<&str, &crate::dialect::Controller> =
        app.controllers.iter().map(|c| (c.name.0.as_str(), c)).collect();
    let mut defined: HashSet<&str> = HashSet::default();
    let mut out = Vec::new();
    let mut prefixes = Vec::new();
    let mut current = by_name.get(class).copied();
    for _ in 0..64 {
        let Some(c) = current else { break };
        if c.name.0.as_str() != class {
            prefixes.push(view_dir(c.name.0.as_str(), false));
        }
        let mut public = true;
        for item in &c.body {
            let action = match item {
                ControllerBodyItem::PrivateMarker { .. } => {
                    public = false;
                    continue;
                }
                ControllerBodyItem::Action { action, .. } => action,
                _ => continue,
            };
            if !defined.insert(action.name.as_str()) {
                continue;
            }
            let from = out.len();
            if public && !ends_in_a_response(&action.body) {
                out.push((action.name.as_str().to_string(), action.body.span));
            }
            collect_relative_renders(&action.body, &mut out);
            if scope.is_some_and(|s| s.suppresses(&action.name)) {
                layout_free.extend(out[from..].iter().map(|(_, span)| *span));
            }
        }
        for m in included_methods(app, c) {
            collect_relative_renders(&m.body, &mut out);
        }
        current = c.parent.as_ref().and_then(|p| by_name.get(p.0.as_str().trim_start_matches("::")).copied());
    }
    (out, prefixes)
}

/// The methods of the modules a controller includes (a concern's actions
/// and helpers), which render through the controller's lookup.
fn included_methods<'a>(app: &'a App, c: &crate::dialect::Controller) -> Vec<&'a crate::dialect::MethodDef> {
    let mut scope: Vec<&str> = c.name.0.as_str().split("::").collect();
    scope.pop();
    let mut out = Vec::new();
    for item in &c.body {
        let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if !matches!(method.as_str(), "include" | "prepend") {
            continue;
        }
        for name in args.iter().filter_map(const_name) {
            let candidates = (0..=scope.len()).rev().map(|n| {
                if n == 0 { name.clone() } else { format!("{}::{name}", scope[..n].join("::")) }
            });
            let module = candidates
                .filter_map(|candidate| app.library_classes.iter().find(|lc| lc.name.0.as_str() == candidate))
                .next();
            out.extend(module.into_iter().flat_map(|lc| &lc.methods));
        }
    }
    out
}

/// Whether a body's last statement responds, leaving nothing to render
/// implicitly.
fn ends_in_a_response(body: &Expr) -> bool {
    let last = match &*body.node {
        ExprNode::Seq { exprs } => exprs.last(),
        _ => Some(body),
    };
    last.is_some_and(|e| {
        matches!(&*e.node, ExprNode::Send { recv: None, method, .. }
            if matches!(method.as_str(), "render" | "redirect_to" | "redirect_back" | "redirect_back_or_to" | "head" | "send_data" | "send_file"))
    })
}

/// `render :show`, `render "show"`, `render action: :show`, `render
/// partial: "form"`: names its controller's prefixes resolve.
fn collect_relative_renders(e: &Expr, out: &mut Vec<(String, Span)>) {
    if let ExprNode::Send { recv: None, method, args, .. } = &*e.node
        && matches!(method.as_str(), "render" | "render_to_string")
    {
        let name_of = |v: &Expr| match &*v.node {
            ExprNode::Lit { value: Literal::Str { value } } if !value.contains('/') => Some(value.clone()),
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
            _ => None,
        };
        for (i, arg) in args.iter().enumerate() {
            match &*arg.node {
                ExprNode::Hash { entries, .. } => {
                    for (k, v) in entries {
                        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { continue };
                        let Some(name) = name_of(v) else { continue };
                        match key.as_str() {
                            "action" | "template" => out.push((name, e.span)),
                            "partial" => out.push((format!("_{}", name.trim_start_matches('_')), e.span)),
                            _ => {}
                        }
                    }
                }
                _ if i == 0 => {
                    if let Some(name) = name_of(arg) {
                        out.push((name, e.span));
                    }
                }
                _ => {}
            }
        }
    }
    e.node.for_each_child(&mut |c| collect_relative_renders(c, out));
}

fn collect_controller_renders(e: &Expr, out: &mut Vec<(String, Span)>) {
    if let ExprNode::Send { recv: None, method, args, .. } = &*e.node
        && matches!(method.as_str(), "render" | "render_to_string")
    {
        render_names(args, e.span, out);
    }
    e.node.for_each_child(&mut |c| collect_controller_renders(c, out));
}

/// `PagesController.render(...)` / `ApplicationController.renderer.render(...)`
/// anywhere: a render through that controller's lookup, by its name.
fn collect_class_renders(e: &Expr, out: &mut Vec<(String, (String, Span))>) {
    if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node
        && matches!(method.as_str(), "render" | "render_to_string")
    {
        let class = match &*recv.node {
            ExprNode::Send { recv: Some(inner), method, args, .. } if method.as_str() == "renderer" && args.is_empty() => {
                const_name(inner)
            }
            _ => const_name(recv),
        };
        if let Some(class) = class {
            let mut names = Vec::new();
            render_names(args, e.span, &mut names);
            out.extend(names.into_iter().map(|n| (class.clone(), n)));
        }
    }
    e.node.for_each_child(&mut |c| collect_class_renders(c, out));
}

/// Each render's `layout: false` / `layout: nil` and `formats:` / `format:`.
fn collect_render_options(e: &Expr, out: &mut HashMap<Span, RenderOptions>) {
    if let ExprNode::Send { method, args, .. } = &*e.node
        && matches!(method.as_str(), "render" | "render_to_string")
    {
        let mut options = RenderOptions::default();
        for arg in args {
            let ExprNode::Hash { entries, .. } = &*arg.node else { continue };
            for (k, v) in entries {
                let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { continue };
                let name = |e: &Expr| match &*e.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                    ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                    _ => None,
                };
                match key.as_str() {
                    "layout" => {
                        options.without_layout =
                            matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: false } } | ExprNode::Lit { value: Literal::Nil })
                    }
                    "formats" | "format" => {
                        options.formats = match &*v.node {
                            ExprNode::Array { elements, .. } => elements.iter().map(name).collect(),
                            _ => name(v).map(|f| vec![f]),
                        }
                    }
                    _ => {}
                }
            }
        }
        if options.without_layout || options.formats.is_some() {
            out.insert(e.span, options);
        }
    }
    e.node.for_each_child(&mut |c| collect_render_options(c, out));
}

/// The templates a render's arguments name with a directory.
fn render_names(args: &[Expr], span: Span, out: &mut Vec<(String, Span)>) {
    for arg in args {
        match &*arg.node {
            ExprNode::Lit { value: Literal::Str { value } } if value.contains('/') => out.push((value.clone(), span)),
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    let (ExprNode::Lit { value: Literal::Sym { value: key } }, ExprNode::Lit { value: Literal::Str { value } }) =
                        (&*k.node, &*v.node)
                    else {
                        continue;
                    };
                    match key.as_str() {
                        "template" if value.contains('/') => out.push((value.clone(), span)),
                        "partial" if value.contains('/') => out.push((partial_name(value, ""), span)),
                        "layout" => out.push((format!("layouts/{value}"), span)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// A mailer's layouts, as `lower::mailer_templates` finds them: the nearest
/// `layout "x"`, and each class's own directory's under `layouts/`.
fn mailer_layouts(app: &App, class: &str) -> Vec<String> {
    let by_name: HashMap<&str, &crate::dialect::LibraryClass> =
        app.library_classes.iter().map(|lc| (lc.name.0.as_str(), lc)).collect();
    let mut out = Vec::new();
    let mut current = by_name.get(class).copied();
    for _ in 0..64 {
        let Some(lc) = current else { break };
        out.push(format!("layouts/{}", view_dir(lc.name.0.as_str(), true)));
        for call in &lc.unknown_calls {
            if let ExprNode::Send { recv: None, method, args, .. } = &*call.node
                && method.as_str() == "layout"
                && let [arg] = args.as_slice()
                && let ExprNode::Lit { value: Literal::Str { value } } = &*arg.node
            {
                out.push(format!("layouts/{value}"));
            }
        }
        current = lc.parent.as_ref().and_then(|p| by_name.get(p.0.as_str()).copied());
    }
    out
}

/// The templates a template's renders name, as the view lowering resolves
/// them: a bare name in the template's own directory, a record by its
/// plural directory.
fn template_renders(body: &Expr, dir: &str) -> Vec<(String, Span)> {
    fn walk(e: &Expr, dir: &str, out: &mut Vec<(String, Span)>) {
        use crate::lower::view::RenderPartial;
        if let ExprNode::Send { recv, method, args, block, .. } = &*e.node
            && let Some(rp) = crate::lower::view::classify_render_partial(
                recv.as_ref(),
                method.as_str(),
                args,
                block.as_ref(),
                &|_| true,
                &|_| false,
            )
        {
            let name = match rp {
                RenderPartial::Named { partial, .. } | RenderPartial::CollectionNamed { partial, .. } => {
                    Some(partial_name(partial, dir))
                }
                RenderPartial::LayoutBlock { layout, .. } => Some(partial_name(layout, dir)),
                RenderPartial::Template { name } if name.contains('/') => Some(name.to_string()),
                RenderPartial::Template { name } => Some(format!("{dir}/{name}")),
                RenderPartial::Record { name, .. } => Some(format!("{}/_{name}", crate::naming::pluralize_snake(name))),
                RenderPartial::Collection { name, .. } => Some(format!("{name}/_{}", crate::naming::singularize(name))),
                RenderPartial::Association { method, .. } => {
                    Some(format!("{method}/_{}", crate::naming::singularize(method)))
                }
                RenderPartial::TypedCollection { partial, .. } => Some(partial_name(&partial, dir)),
                RenderPartial::DynamicNamed { .. } => None,
            };
            if let Some(name) = name {
                out.push((name, e.span));
            }
        }
        e.node.for_each_child(&mut |c| walk(c, dir, out));
    }
    let mut out = Vec::new();
    walk(body, dir, &mut out);
    out
}

/// `"form"` in `pages` → `pages/_form`; `"shared/nav"` → `shared/_nav`.
fn partial_name(partial: &str, dir: &str) -> String {
    match partial.rsplit_once('/') {
        Some((d, n)) => format!("{d}/_{}", n.trim_start_matches('_')),
        None => format!("{dir}/_{}", partial.trim_start_matches('_')),
    }
}

/// Every string the app's code could render a template by: each literal,
/// and the literal head of each interpolated string.
struct RenderLiterals {
    exact: HashSet<String>,
    heads: Vec<String>,
}

impl RenderLiterals {
    fn collect(app: &App, ingested: &HashMap<PathBuf, View>) -> Self {
        let mut lits = RenderLiterals { exact: HashSet::default(), heads: Vec::new() };
        let mut visit = |e: &Expr| lits.walk(e);
        for c in &app.controllers {
            for item in &c.body {
                match item {
                    ControllerBodyItem::Action { action, .. } => visit(&action.body),
                    ControllerBodyItem::Unknown { expr, .. } => visit(expr),
                    _ => {}
                }
            }
        }
        for m in app.library_classes.iter().flat_map(|lc| &lc.methods).chain(app.models.iter().flat_map(|m| m.methods())) {
            visit(&m.body);
        }
        for view in ingested.values() {
            visit(&view.body);
        }
        lits.heads.sort();
        lits.heads.dedup();
        lits
    }

    fn walk(&mut self, e: &Expr) {
        match &*e.node {
            ExprNode::Lit { value: Literal::Str { value } } => {
                self.exact.insert(value.clone());
            }
            ExprNode::StringInterp { parts } => {
                if let Some(InterpPart::Text { value }) = parts.first()
                    && value.contains('/')
                {
                    self.heads.push(value.clone());
                }
            }
            _ => {}
        }
        e.node.for_each_child(&mut |c| self.walk(c));
    }

    /// Whether some string names the template `name`
    /// (`themes/next/pages/_form` is named by `"themes/next/pages/form"` too).
    fn names(&self, name: &str) -> bool {
        let unprefixed = match name.rsplit_once('/') {
            Some((dir, base)) => format!("{dir}/{}", base.trim_start_matches('_')),
            None => name.to_string(),
        };
        self.exact.contains(name)
            || self.exact.contains(&unprefixed)
            || self.heads.iter().any(|h| name.starts_with(h.as_str()) || unprefixed.starts_with(h.as_str()))
    }
}
