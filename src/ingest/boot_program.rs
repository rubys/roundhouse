//! The app's own boot code in `config/initializers/*.rb`, run at boot on
//! the ruby family and spinel (E2 of rubys/roundhouse#841, for the host's
//! initializers).
//!
//! Rails `load`s every initializer once at boot, in path order, after the
//! app's classes are autoloadable. A statement no recognizer reads and
//! whose holder is the app itself (`initializer_ledger::Holder::App`) is
//! the app's code: a constant it assigns, a loop over files, a log line,
//! a call. Those become [`App::boot_program`], typed like `db/seeds.rb`
//! and emitted as top-level code that the tree's `boot.rb` loads last.
//!
//! A top-level `def` defines a private method on Object, callable from
//! every class. Here it becomes a class method of the synthesized
//! [`MODULE`], and each receiverless call to it names that module
//! (`apply_call_sites`), so every target reaches it as an ordinary typed
//! method.
//!
//! Left to the ledger, as warnings: statements of any other holder (a
//! Rails or gem setting needs that framework's behaviour, not a replay),
//! statements nested in a `configure` block or a `to_prepare` body, and
//! the boot hooks themselves (`after_initialize`, `on_load`, …), whose
//! run order is the rest of #841's E2.

use std::path::Path;

use crate::app::App;
use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::vfs::Vfs;

use super::initializer_ledger::{self, Holder};
use super::IngestResult;
use super::prism_api::PrismLocation;

/// The module an initializer's top-level `def`s become class methods of.
pub const MODULE: &str = "RoundhouseInitializers";

const HOOKS: &[&str] = &[
    "after_initialize", "before_initialize", "to_prepare", "to_run", "to_complete", "on_load",
    "after_routes_loaded", "configure", "setup",
];

/// The host initializer statements that became the boot program or a
/// [`MODULE`] method, by `(path, start)` as the ledger reports them.
pub(super) type Taken = crate::det::HashSet<(String, u32)>;

pub(super) fn build<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    app: &mut App,
    lock: Option<&crate::gems::Lockfile>,
    app_namespaces: &crate::det::HashSet<String>,
    carried_rate_limit_stores: &[crate::span::Span],
) -> IngestResult<Taken> {
    let mut taken = Taken::default();
    let mut module = LibraryClass {
        name: ClassId(Symbol::from(MODULE)),
        is_module: true,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        include_spans: Vec::new(),
        methods: Vec::new(),
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    };
    let no_engines = crate::det::HashSet::default();
    // Top-level constants an earlier boot statement assigned: this program's own, free to fill.
    let mut defined: crate::det::HashSet<String> = crate::det::HashSet::default();
    for file in initializer_ledger::ledger(vfs, dir, &[], &no_engines) {
        let wanted: Vec<(String, u32)> = file
            .findings
            .iter()
            .filter(|f| f.path.starts_with("config/initializers/"))
            .filter(|f| matches!(initializer_ledger::holder_of(&f.targets, lock, app_namespaces), Holder::App))
            .map(|f| (f.path.clone(), f.start))
            .collect();
        let Some((path, _)) = wanted.first() else { continue };
        let full = dir.join(path);
        let Ok(bytes) = vfs.read(&full) else { continue };
        let file_name = full.display().to_string();
        let file_id = super::sources::file_id(&file_name);
        let parsed = super::prism::parse_silent(&bytes);
        let root = parsed.node();
        let Some(program) = root.as_program_node() else { continue };
        for node in program.statements().body().iter() {
            let start = node.location().start_offset() as u32;
            if !wanted.iter().any(|(_, s)| *s == start) || is_hook(&node) {
                continue;
            }
            let end = node.location().end_offset() as u32;
            // rate_limit already moved this store onto Object; replaying it here would build a second store.
            if carried_rate_limit_stores.iter().any(|value| value.file == file_id && value.start >= start && value.end <= end) {
                continue;
            }
            if node.as_def_node().is_none() && reshapes_the_program(&node, &defined) {
                continue;
            }
            // Not the branches alone, which is all the ledger weighs for a conditional: the condition runs at boot too.
            if let Some(predicate) = predicate_of(&node) {
                let loc = predicate.location();
                let text = String::from_utf8_lossy(&bytes[loc.start_offset()..loc.end_offset()]).into_owned();
                if !constant_roots(&text).iter().all(|root| root == "Rails" || root == "ENV" || app_namespaces.contains(root)) {
                    continue;
                }
            }
            if let Some(def) = node.as_def_node() {
                if def.receiver().is_some() {
                    continue;
                }
                let Ok(mut method) = super::library_class::ingest_library_method_with_keywords(
                    &def,
                    &module.name,
                    &file_name,
                    true,
                ) else {
                    continue;
                };
                method.receiver = MethodReceiver::Class;
                module.methods.push(method);
            } else {
                // Not the ingest error: a statement this cannot read stays on the ledger, as before.
                let (read, _) = super::prism::scope(|| super::survey::without_recording(|| super::expr::ingest_expr(&node, &file_name)));
                let Ok(expr) = read else { continue };
                if let Some(write) = node.as_constant_write_node() {
                    defined.insert(super::util::constant_id_str(&write.name()).to_string());
                }
                if let Some(write) = node.as_constant_path_write_node() {
                    if let Some(name) = write.target().name() {
                        defined.insert(super::util::constant_id_str(&name).to_string());
                    }
                }
                app.boot_program.push(expr);
            }
            taken.insert((path.clone(), start));
        }
    }
    if !module.methods.is_empty() {
        app.library_classes.push(module);
    }
    Ok(taken)
}

/// The first segment of each constant the source names (`SolidCache` in
/// `SolidCache.configuration`, `Foo` in `Foo::Bar`), skipping string contents.
fn constant_roots(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            i += 1;
            continue;
        }
        let starts = c.is_ascii_uppercase()
            && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == '.' || chars[i - 1] == ':' || chars[i - 1] == '@' || chars[i - 1] == '$'));
        if starts {
            let mut j = i;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            out.push(chars[i..j].iter().collect());
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

fn predicate_of<'a>(node: &ruby_prism::Node<'a>) -> Option<ruby_prism::Node<'a>> {
    if let Some(n) = node.as_if_node() {
        return Some(n.predicate());
    }
    node.as_unless_node().map(|n| n.predicate())
}

/// Does the statement change the program rather than run in it: define a
/// class, module or method, write into an existing namespace's constant,
/// mutate a constant in place, or reopen a class by reflection? That is
/// the rest of #841 (E4's folds and the reopen rules), so it stays on the
/// ledger.
fn reshapes_the_program(node: &ruby_prism::Node<'_>, defined: &crate::det::HashSet<String>) -> bool {
    struct Finder<'d> {
        found: bool,
        defined: &'d crate::det::HashSet<String>,
    }
    const REFLECTION: &[&str] = &[
        "send", "public_send", "__send__", "class_eval", "module_eval", "instance_eval", "class_exec",
        "module_exec", "instance_exec", "const_set", "remove_const", "define_method", "define_singleton_method",
        "alias_method", "remove_method", "undef_method", "include", "prepend", "extend",
    ];
    const MUTATORS: &[&str] = &[
        "<<", "push", "append", "unshift", "prepend", "concat", "replace", "clear", "delete", "delete_if",
        "reject!", "select!", "keep_if", "merge!", "update", "store", "[]=", "insert", "fill", "map!", "sort!",
    ];
    impl<'pr> ruby_prism::Visit<'pr> for Finder<'_> {
        fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {
            self.found = true;
        }
        fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {
            self.found = true;
        }
        fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {
            self.found = true;
        }
        fn visit_singleton_class_node(&mut self, _: &ruby_prism::SingletonClassNode<'pr>) {
            self.found = true;
        }
        fn visit_constant_path_write_node(&mut self, node: &ruby_prism::ConstantPathWriteNode<'pr>) {
            if node.target().parent().is_some() {
                self.found = true;
            }
            ruby_prism::visit_constant_path_write_node(self, node);
        }
        fn visit_constant_path_operator_write_node(&mut self, _: &ruby_prism::ConstantPathOperatorWriteNode<'pr>) {
            self.found = true;
        }
        fn visit_constant_path_or_write_node(&mut self, _: &ruby_prism::ConstantPathOrWriteNode<'pr>) {
            self.found = true;
        }
        fn visit_constant_operator_write_node(&mut self, _: &ruby_prism::ConstantOperatorWriteNode<'pr>) {
            self.found = true;
        }
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let name = super::util::constant_id_str(&node.name());
            let on_constant = node.receiver().is_some_and(|r| match r.as_constant_read_node() {
                Some(c) => !self.defined.contains(super::util::constant_id_str(&c.name()).as_ref() as &str),
                None => r.as_constant_path_node().is_some(),
            });
            if REFLECTION.contains(&name.as_ref()) || (on_constant && MUTATORS.contains(&name.as_ref())) {
                self.found = true;
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let mut finder = Finder { found: false, defined };
    ruby_prism::Visit::visit(&mut finder, node);
    finder.found
}

/// A boot hook call (`Rails.application.config.after_initialize do … end`):
/// its block runs at a point in boot this program does not reproduce.
fn is_hook(node: &ruby_prism::Node<'_>) -> bool {
    node.as_call_node().is_some_and(|c| {
        HOOKS.contains(&super::util::constant_id_str(&c.name()).as_ref()) && c.block().is_some()
    })
}

/// Point each receiverless call to an initializer `def` at [`MODULE`], in
/// every body a class, a view or the boot program holds, unless the class
/// defines a method of that name itself.
pub(super) fn apply_call_sites(app: &mut App) {
    let names: Vec<Symbol> = app
        .library_classes
        .iter()
        .find(|lc| lc.name.0.as_str() == MODULE)
        .map(|lc| lc.methods.iter().map(|m| m.name.clone()).collect())
        .unwrap_or_default();
    if names.is_empty() {
        return;
    }
    fn visit(e: &mut Expr, names: &[Symbol], own: &dyn Fn(&Symbol) -> bool) {
        e.node.for_each_child_mut(&mut |c| visit(c, names, own));
        if let ExprNode::Send { recv, method, .. } = &mut *e.node {
            if recv.is_none() && names.contains(method) && !own(method) {
                let mut module = Expr::new(e.span, ExprNode::Const { path: vec![Symbol::from(MODULE)] });
                module.decisions |= crate::expr::UNINDEXED_SCOPE_CONST_REF;
                *recv = Some(module);
            }
        }
    }
    let methods = crate::analyze::SourceMethodIndex::new(app);
    let defines: Vec<(ClassId, Symbol)> = names
        .iter()
        .flat_map(|n| {
            app.controllers.iter().map(|c| c.name.clone())
                .chain(app.models.iter().map(|m| m.name.clone()))
                .chain(app.library_classes.iter().filter(|lc| lc.name.0.as_str() != MODULE).map(|lc| lc.name.clone()))
                .filter(|owner| methods.instance_method(owner, n.as_str()))
                .map(move |owner| (owner, n.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    drop(methods);
    crate::lower::for_each_owned_hook_body(app, &mut |owner, body| {
        let owner = owner.cloned();
        visit(body, &names, &|name| owner.as_ref().is_some_and(|o| defines.iter().any(|(c, n)| c == o && n == name)));
    });
    for view in &mut app.views {
        visit(&mut view.body, &names, &|_| false);
    }
}
