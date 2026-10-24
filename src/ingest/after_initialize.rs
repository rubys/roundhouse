//! `after_initialize` callbacks, run at boot as analyzed code.
//!
//! Rails runs every `after_initialize` block once the application has
//! booted: the blocks `config/application.rb` registers in its class
//! body first, then those of `config/initializers/*.rb` in file order.
//! Until this pass the tree ran none of them and the initializer ledger
//! reported each as a warning, so an app whose callback patches a
//! module (`Lib.module_eval do alias_method :original_find, :find; def
//! find(...) … end end`) compiled cleanly and answered differently.
//!
//! WHAT IS CARRIED. A block with no parameters whose body reads no
//! local of the code around it (the body runs as a method, which has
//! neither). The spellings: `config.after_initialize` and a bare
//! `after_initialize` in the `Rails::Application` class body;
//! `Rails.application.config.after_initialize` (or
//! `Rails.configuration.after_initialize`) at the top of an initializer;
//! `config.after_initialize` inside `Rails.application.configure do …
//! end` there. Any other block stays unread, and the ledger reports it.
//!
//! HOW. Each body is ingested from its own source like any method body
//! (real spans) and becomes a class method of the generated
//! [`AFTER_INITIALIZE_RUNNER`] module; its `run` calls them in order and
//! the emitted `boot.rb` calls `run` once everything is loaded, on the
//! Ruby family and on Spinel alike (`project::apply_app_initializers`).
//! `diagnose` walks these bodies with the policy of a model method, not
//! the lenient library one, so a call with no runtime behind it is an
//! error, as it would be in the app's own code. A `next` at the body's
//! top level leaves the callback, as it leaves the block in Rails: it
//! becomes a `return`.
//!
//! STATIC PARTS. Two statements at a body's top level are not run but
//! applied while ingesting, because they change what the program is:
//! * `Const.module_eval do … end` (or `class_eval` / `module_exec` /
//!   `class_exec`): a reopen. When `Const` is a library class or module
//!   of the compiled program, its `def`s replace or join that module's
//!   methods and an `alias_method :new, :old` copies `old` as it stands,
//!   before a later `def old` replaces it, as Ruby's alias does. Any
//!   other receiver (a gem, framework, model or controller constant) is
//!   an ingest error: the patch would silently not apply.
//! * `require` of an app file (a literal path under `lib/`, or
//!   `Rails.root.join("lib/…").to_s`): the file's top-level statements
//!   are read here, under the same rules, as `require` runs them here.
//!   Any other `require` is an error: the emitted program loads no files
//!   at run time.
//!
//! Applying a reopen statically means code that runs during boot, before
//! the callbacks fire, already sees the patched method. Nothing emitted
//! runs app code at that point.

use std::path::{Path, PathBuf};

use ruby_prism::Node;

use crate::app::{App, AFTER_INITIALIZE_RUNNER};
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ingest::prism_api::PrismLocation;
use crate::span::Span;
use crate::vfs::Vfs;

use super::util::{constant_id_str, constant_path_of, find_all_classes_with_scope, flatten_statements, symbol_value};
use super::{survey, IngestError, IngestResult};

/// Where an `after_initialize` call sits, which decides the receivers it
/// can be spelled with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Site {
    /// The top level of a `config/initializers` file.
    Initializer,
    /// Inside `Rails.application.configure do … end` in an initializer.
    Configure,
    /// The `Rails::Application` subclass body of `config/application.rb`.
    Application,
}

/// The block of an `after_initialize` call this pass carries, or `None`.
/// The initializer ledger asks with the same node, parsed with its whole
/// file, so a local of the code around the block is told apart from a
/// method call.
pub(super) fn carried_block<'pr>(call: &ruby_prism::CallNode<'pr>, site: Site) -> Option<ruby_prism::BlockNode<'pr>> {
    if constant_id_str(&call.name()) != "after_initialize" || call.arguments().is_some() {
        return None;
    }
    let receiver = call.receiver().map(|r| {
        String::from_utf8_lossy(r.location().as_slice()).chars().filter(|c| !c.is_whitespace()).collect::<String>()
    });
    let spelled = match (site, receiver.as_deref()) {
        (Site::Initializer, Some(r)) => matches!(r, "Rails.application.config" | "Rails.configuration"),
        (Site::Configure, Some(r)) => matches!(r, "config" | "Rails.application.config" | "Rails.configuration"),
        (Site::Application, None) => true,
        (Site::Application, Some(r)) => r == "config",
        _ => false,
    };
    if !spelled {
        return None;
    }
    let block = call.block()?.as_block_node()?;
    if block.parameters().is_some() {
        return None;
    }
    let body = block.body()?;
    let mut outer = OuterLocals { nesting: 0, found: false };
    ruby_prism::Visit::visit(&mut outer, &body);
    (!outer.found).then_some(block)
}

/// Whether a body reads or writes a local of the code around its block:
/// a variable whose depth is past the blocks the body itself opens.
struct OuterLocals {
    nesting: u32,
    found: bool,
}

impl<'pr> ruby_prism::Visit<'pr> for OuterLocals {
    fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'pr>) {
        self.nesting += 1;
        ruby_prism::visit_block_node(self, node);
        self.nesting -= 1;
    }
    fn visit_lambda_node(&mut self, node: &ruby_prism::LambdaNode<'pr>) {
        self.nesting += 1;
        ruby_prism::visit_lambda_node(self, node);
        self.nesting -= 1;
    }
    fn visit_local_variable_read_node(&mut self, node: &ruby_prism::LocalVariableReadNode<'pr>) {
        self.found |= node.depth() > self.nesting;
    }
    fn visit_local_variable_target_node(&mut self, node: &ruby_prism::LocalVariableTargetNode<'pr>) {
        self.found |= node.depth() > self.nesting;
    }
    fn visit_local_variable_write_node(&mut self, node: &ruby_prism::LocalVariableWriteNode<'pr>) {
        self.found |= node.depth() > self.nesting;
        ruby_prism::visit_local_variable_write_node(self, node);
    }
    fn visit_local_variable_operator_write_node(&mut self, node: &ruby_prism::LocalVariableOperatorWriteNode<'pr>) {
        self.found |= node.depth() > self.nesting;
        ruby_prism::visit_local_variable_operator_write_node(self, node);
    }
    fn visit_local_variable_and_write_node(&mut self, node: &ruby_prism::LocalVariableAndWriteNode<'pr>) {
        self.found |= node.depth() > self.nesting;
        ruby_prism::visit_local_variable_and_write_node(self, node);
    }
    fn visit_local_variable_or_write_node(&mut self, node: &ruby_prism::LocalVariableOrWriteNode<'pr>) {
        self.found |= node.depth() > self.nesting;
        ruby_prism::visit_local_variable_or_write_node(self, node);
    }
}

/// One carried callback: what it runs, and the reopens it applies.
pub(super) struct Callback {
    span: Span,
    body: Vec<Expr>,
    reopens: Vec<Reopen>,
}

/// `Const.module_eval do … end` inside a callback.
struct Reopen {
    target: String,
    via: String,
    file: String,
    items: Vec<ReopenItem>,
}

enum ReopenItem {
    Def(MethodDef),
    Alias { to: Symbol, from: Symbol, span: Span },
}

fn unsupported(file: &str, message: String) -> IngestError {
    IngestError::Unsupported { file: file.into(), message }
}

fn span_of(node: &Node<'_>, file: &str) -> Span {
    let loc = node.location();
    Span { file: super::sources::file_id(file), start: loc.start_offset() as u32, end: loc.end_offset() as u32 }
}

/// Every carried callback of the app, in the order Rails runs them:
/// `config/application.rb` first, then the initializers by path. A
/// callback that fails to read is recorded (survey) or fails the ingest.
pub(super) fn read_all<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<Vec<Callback>> {
    let mut out = Vec::new();
    let application = dir.join("config/application.rb");
    if let Ok(bytes) = vfs.read(&application) {
        let file = application.display().to_string();
        super::sources::register(&file, &String::from_utf8_lossy(&bytes));
        let parsed = super::prism::parse(&bytes, &file);
        let root = parsed.node();
        for (_, class) in find_all_classes_with_scope(&root) {
            let is_application = class
                .superclass()
                .and_then(|s| constant_path_of(&s))
                .is_some_and(|p| p.join("::").trim_start_matches("::") == "Rails::Application");
            let Some(body) = class.body().filter(|_| is_application) else { continue };
            for stmt in flatten_statements(body) {
                read_statement(vfs, dir, &stmt, &file, Site::Application, &mut out)?;
            }
        }
    }
    let init_dir = dir.join("config/initializers");
    if vfs.is_dir(&init_dir) {
        for entry in super::app::read_rb_files(vfs, &init_dir)? {
            let Ok(bytes) = vfs.read(&entry) else { continue };
            let file = entry.display().to_string();
            super::sources::register(&file, &String::from_utf8_lossy(&bytes));
            let parsed = super::prism::parse(&bytes, &file);
            let root = parsed.node();
            let Some(program) = root.as_program_node() else { continue };
            for stmt in program.statements().body().iter() {
                read_statement(vfs, dir, &stmt, &file, Site::Initializer, &mut out)?;
            }
        }
    }
    Ok(out)
}

/// One statement of a file: a carried callback, or a `configure` block
/// whose statements are read in turn.
fn read_statement<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    stmt: &Node<'_>,
    file: &str,
    site: Site,
    out: &mut Vec<Callback>,
) -> IngestResult<()> {
    let Some(call) = stmt.as_call_node() else { return Ok(()) };
    if site == Site::Initializer && constant_id_str(&call.name()) == "configure" {
        let receiver = call.receiver().map(|r| String::from_utf8_lossy(r.location().as_slice()).into_owned());
        if matches!(receiver.as_deref(), None | Some("Rails.application")) {
            if let Some(body) = call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body()) {
                for inner in flatten_statements(body) {
                    read_statement(vfs, dir, &inner, file, Site::Configure, out)?;
                }
            }
        }
        return Ok(());
    }
    let Some(block) = carried_block(&call, site) else { return Ok(()) };
    let mut callback = Callback { span: span_of(stmt, file), body: Vec::new(), reopens: Vec::new() };
    let read = block
        .body()
        .map(|body| read_body(vfs, dir, flatten_statements(body), file, &mut callback, 0))
        .unwrap_or(Ok(()));
    match read {
        Ok(()) => out.push(callback),
        Err(err) => survey::continue_or_fail(err)?,
    }
    Ok(())
}

/// A callback body's top-level statements (or a required file's): the
/// static parts applied, the rest ingested to run.
fn read_body<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    stmts: Vec<Node<'_>>,
    file: &str,
    callback: &mut Callback,
    depth: usize,
) -> IngestResult<()> {
    for stmt in stmts {
        if let Some(reopen) = reopen(&stmt, file)? {
            callback.reopens.push(reopen);
            continue;
        }
        if let Some(required) = required_app_file(vfs, dir, &stmt, file)? {
            if depth > 8 {
                return Err(unsupported(file, "require nesting inside an after_initialize callback is too deep".into()));
            }
            let bytes = vfs.read(&required)?;
            let path = required.display().to_string();
            super::sources::register(&path, &String::from_utf8_lossy(&bytes));
            let parsed = super::prism::parse(&bytes, &path);
            let root = parsed.node();
            let Some(program) = root.as_program_node() else { continue };
            let inner: Vec<Node<'_>> = program.statements().body().iter().collect();
            if inner.iter().any(|n| n.as_class_node().is_some() || n.as_module_node().is_some()) {
                return Err(unsupported(
                    &path,
                    "a file required from an after_initialize callback defines a class or module; only its top-level statements are read".into(),
                ));
            }
            read_body(vfs, dir, inner, &path, callback, depth + 1)?;
            continue;
        }
        let mut expr = super::expr::ingest_expr(&stmt, file)?;
        if let Some(name) = dynamic_construct(&expr) {
            return Err(unsupported(
                file,
                format!("`{name}` inside an after_initialize callback is not modeled except as a top-level statement of the block"),
            ));
        }
        next_to_return(&mut expr);
        callback.body.push(expr);
    }
    Ok(())
}

const REOPENERS: &[&str] = &["module_eval", "class_eval", "module_exec", "class_exec"];
const LOADERS: &[&str] = &["require", "require_relative", "load"];

/// A reopen or a load left where nothing applies it statically: the
/// emitted program cannot run it.
fn dynamic_construct(expr: &Expr) -> Option<String> {
    if let ExprNode::Send { method, recv, .. } = &*expr.node {
        let name = method.as_str();
        if REOPENERS.contains(&name) || (recv.is_none() && LOADERS.contains(&name)) {
            return Some(name.to_string());
        }
    }
    let mut found = None;
    expr.node.for_each_child(&mut |child| {
        if found.is_none() {
            found = dynamic_construct(child);
        }
    });
    found
}

/// `next` at the callback's own level leaves it; in a method that is a
/// `return`. A `next` inside a block or a loop of the body stays.
fn next_to_return(expr: &mut Expr) {
    match &mut *expr.node {
        ExprNode::Lambda { .. } | ExprNode::While { .. } => return,
        ExprNode::Next { value } => {
            let value = value.take().unwrap_or_else(|| Expr::new(expr.span, ExprNode::Lit { value: Literal::Nil }));
            *expr.node = ExprNode::Return { value };
            return;
        }
        _ => {}
    }
    expr.node.for_each_child_mut(&mut |child| next_to_return(child));
}

/// `Const.module_eval do … end` as a reopen, or `None` for any other
/// statement.
fn reopen(stmt: &Node<'_>, file: &str) -> IngestResult<Option<Reopen>> {
    let Some(call) = stmt.as_call_node() else { return Ok(None) };
    let via = constant_id_str(&call.name());
    if !REOPENERS.contains(&via) {
        return Ok(None);
    }
    let Some(target) = call.receiver().and_then(|r| constant_path_of(&r)) else { return Ok(None) };
    let target = target.join("::").trim_start_matches("::").to_string();
    let Some(block) = call.block().and_then(|b| b.as_block_node()) else { return Ok(None) };
    if call.arguments().is_some() || block.parameters().is_some() {
        return Err(unsupported(file, format!("`{target}.{via}` with arguments or block parameters in an after_initialize callback")));
    }
    let owner = ClassId(Symbol::from(target.as_str()));
    let mut items = Vec::new();
    for inner in block.body().map(flatten_statements).unwrap_or_default() {
        if let Some(def) = inner.as_def_node() {
            items.push(ReopenItem::Def(super::library_class::ingest_library_method(&def, &owner, file)?));
            continue;
        }
        let alias = if let Some(alias) = inner.as_alias_method_node() {
            super::library_class::alias_keyword_name(&alias.new_name())
                .zip(super::library_class::alias_keyword_name(&alias.old_name()))
        } else if let Some(call) = inner.as_call_node().filter(|c| {
            c.receiver().is_none() && constant_id_str(&c.name()) == "alias_method"
        }) {
            let args: Vec<Option<String>> =
                call.arguments().map(|a| a.arguments().iter().map(|n| symbol_value(&n)).collect()).unwrap_or_default();
            match args.as_slice() {
                [Some(to), Some(from)] => Some((to.clone(), from.clone())),
                _ => None,
            }
        } else {
            None
        };
        match alias {
            Some((to, from)) => items.push(ReopenItem::Alias {
                to: Symbol::from(to.as_str()),
                from: Symbol::from(from.as_str()),
                span: span_of(&inner, file),
            }),
            None => {
                return Err(unsupported(
                    file,
                    format!("only `def` and `alias_method` are read in a `{target}.{via}` reopen inside an after_initialize callback"),
                ))
            }
        }
    }
    Ok(Some(Reopen { target, via: via.to_string(), file: file.to_string(), items }))
}

/// The app file a top-level `require` statement loads, or `None` for a
/// statement that is not a `require`. A `require` of anything else is an
/// error.
fn required_app_file<V: Vfs + ?Sized>(vfs: &V, dir: &Path, stmt: &Node<'_>, file: &str) -> IngestResult<Option<PathBuf>> {
    let Some(call) = stmt.as_call_node() else { return Ok(None) };
    let kind = constant_id_str(&call.name());
    if call.receiver().is_some() || !matches!(kind, "require" | "require_relative") {
        return Ok(None);
    }
    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let text = String::from_utf8_lossy(call.location().as_slice()).into_owned();
    let refuse = || unsupported(file, format!("`{text}` inside an after_initialize callback names no app file the compiled program carries"));
    let [arg] = args.as_slice() else { return Err(refuse()) };
    let relative = if let Some(s) = arg.as_string_node() {
        let path = String::from_utf8_lossy(s.unescaped()).into_owned();
        if kind == "require_relative" {
            Path::new(file).parent().map(|p| p.join(&path)).ok_or_else(refuse)?
        } else {
            dir.join("lib").join(path)
        }
    } else {
        dir.join(rails_root_path(arg).ok_or_else(refuse)?)
    };
    let with_rb = if relative.extension().is_some_and(|e| e == "rb") {
        relative
    } else {
        PathBuf::from(format!("{}.rb", relative.display()))
    };
    if vfs.read(&with_rb).is_err() {
        return Err(refuse());
    }
    Ok(Some(with_rb))
}

/// `Rails.root.join("a", "b").to_s` (or without `.to_s`): the path
/// relative to the app root, when every argument is a string literal.
fn rails_root_path(node: &Node<'_>) -> Option<PathBuf> {
    let mut call = node.as_call_node()?;
    if constant_id_str(&call.name()) == "to_s" && call.arguments().is_none() {
        call = call.receiver()?.as_call_node()?;
    }
    if constant_id_str(&call.name()) != "join" || call.receiver()?.location().as_slice() != b"Rails.root" {
        return None;
    }
    let mut path = PathBuf::new();
    for arg in call.arguments()?.arguments().iter() {
        path.push(String::from_utf8_lossy(arg.as_string_node()?.unescaped()).into_owned());
    }
    Some(path)
}

/// Root constant names the callback bodies read, so a class an
/// initializer defines and only a callback names is kept.
pub(super) fn named_constants(callbacks: &[Callback]) -> std::collections::HashSet<String> {
    fn visit(expr: &Expr, out: &mut std::collections::HashSet<String>) {
        if let ExprNode::Const { path } = &*expr.node {
            if let Some(root) = path.iter().find(|s| !s.as_str().is_empty()) {
                out.insert(root.as_str().to_string());
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, out));
    }
    let mut out = std::collections::HashSet::new();
    for callback in callbacks {
        for expr in &callback.body {
            visit(expr, &mut out);
        }
    }
    out
}

/// Apply the reopens to the compiled modules and add the runner module
/// with one class method per callback that has something to run.
pub(super) fn apply(app: &mut App, callbacks: Vec<Callback>) -> IngestResult<()> {
    let runner = ClassId(Symbol::from(AFTER_INITIALIZE_RUNNER));
    let mut methods: Vec<MethodDef> = Vec::new();
    for callback in callbacks {
        for reopen in callback.reopens {
            if let Err(err) = apply_reopen(app, reopen) {
                survey::continue_or_fail(err)?;
            }
        }
        if callback.body.is_empty() {
            continue;
        }
        let name = format!("after_initialize_{}", methods.len() + 1);
        let span = callback.span;
        let body = Expr::new(span, ExprNode::Seq { exprs: callback.body });
        let mut method = crate::runtime_src::parse_methods(&format!("def self.{name}\nend\n"))
            .ok()
            .and_then(|m| m.into_iter().next())
            .ok_or_else(|| unsupported("config/initializers", "after_initialize runner".into()))?;
        method.receiver = MethodReceiver::Class;
        method.enclosing_class = Some(runner.0.clone());
        method.name_span = span;
        method.body = body;
        methods.push(method);
    }
    if methods.is_empty() {
        return Ok(());
    }
    let taken = app.library_classes.iter().any(|c| c.name == runner)
        || app.models.iter().any(|m| m.name == runner)
        || app.controllers.iter().any(|c| c.name == runner);
    if taken {
        return survey::continue_or_fail(unsupported(
            "config/initializers",
            format!("the app defines `{AFTER_INITIALIZE_RUNNER}`, the module its after_initialize callbacks run from"),
        ));
    }
    let calls: String = methods.iter().map(|m| format!("  {}\n", m.name.as_str())).collect();
    let mut run = crate::runtime_src::parse_methods(&format!("def self.run\n{calls}  nil\nend\n"))
        .map_err(|e| unsupported("config/initializers", format!("after_initialize runner: {e}")))?;
    for m in &mut run {
        m.receiver = MethodReceiver::Class;
        m.enclosing_class = Some(runner.0.clone());
    }
    methods.extend(run);
    app.library_classes.push(LibraryClass {
        name: runner,
        is_module: true,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    });
    app.runs_after_initialize = true;
    Ok(())
}

/// One reopen onto the library class(es) of its name, in statement order.
fn apply_reopen(app: &mut App, reopen: Reopen) -> IngestResult<()> {
    let Reopen { target, via, file, items } = reopen;
    let hosts: Vec<usize> = app
        .library_classes
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name.0.as_str() == target)
        .map(|(i, _)| i)
        .collect();
    let Some(&first) = hosts.first() else {
        return Err(unsupported(
            &file,
            format!(
                "an after_initialize callback reopens `{target}` with `{via}`, which is not a library class or module of the compiled program (a gem, framework, model or controller constant): the patch would not be applied"
            ),
        ));
    };
    // The host that defines `name` on `receiver`, last definition winning.
    let find = |app: &App, name: &Symbol, receiver: MethodReceiver| {
        hosts.iter().rev().find_map(|&h| {
            app.library_classes[h]
                .methods
                .iter()
                .rposition(|m| m.name == *name && m.receiver == receiver)
                .map(|i| (h, i))
        })
    };
    for item in items {
        let (method, at) = match item {
            ReopenItem::Def(method) => {
                let at = find(app, &method.name, method.receiver);
                (method, at)
            }
            ReopenItem::Alias { to, from, span } => {
                let Some((h, i)) = find(app, &from, MethodReceiver::Instance) else {
                    return Err(unsupported(
                        &file,
                        format!("`alias_method :{to}, :{from}` in an after_initialize reopen of `{target}`: the compiled `{target}` does not define `{from}`"),
                    ));
                };
                let mut copy = app.library_classes[h].methods[i].clone();
                copy.name = to.clone();
                copy.name_span = span;
                let at = find(app, &to, MethodReceiver::Instance);
                (copy, at)
            }
        };
        match at {
            Some((h, i)) => app.library_classes[h].methods[i] = method,
            None => app.library_classes[first].methods.push(method),
        }
    }
    Ok(())
}
