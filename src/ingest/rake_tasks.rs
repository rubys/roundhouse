//! `lib/tasks/*.rake` that prepend a hook onto
//! `ActiveRecord::Tasks::DatabaseTasks`.
//!
//! The file is kept when the AST is that shape: a module whose
//! `load_schema` / `migrate` call `super`, prepended onto the
//! DatabaseTasks singleton, plus a namespaced task. The generated file
//! copies those method and task bodies. The one rewrite is
//! `singleton_class.prepend`, which Spinel does not put in the ancestor
//! chain, into `class << self; prepend`. `...` on those two methods is
//! expanded to the signatures DatabaseTasks actually has, because Spinel
//! refuses full forwarding. Anything else is named and left out.

use std::path::Path;

use ruby_prism::Node;

use crate::app::App;
use crate::diagnostic::{Diagnostic, Severity};
use crate::span::Span;
use crate::vfs::Vfs;

use crate::ingest::prism_api::PrismLocation;
struct Task {
    name: String,
    prerequisite: String,
    body: String,
}

struct Hook {
    module_name: String,
    module_source: String,
    tasks: Vec<Task>,
}

pub(crate) fn ingest<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> (Option<String>, Vec<String>) {
    let tasks_dir = dir.join("lib/tasks");
    if !vfs.is_dir(&tasks_dir) {
        return (None, Vec::new());
    }
    let Ok(mut entries) = vfs.read_dir(&tasks_dir) else {
        return (None, Vec::new());
    };
    entries.sort();
    let mut hooks = Vec::new();
    let mut gaps = Vec::new();
    for entry in entries {
        if entry.extension().and_then(|ext| ext.to_str()) != Some("rake") {
            continue;
        }
        let rel = entry
            .strip_prefix(dir)
            .unwrap_or(&entry)
            .display()
            .to_string();
        let Ok(text) = vfs.read_to_string(&entry) else {
            gaps.push(format!("`{rel}` was left out of boot: it could not be read"));
            continue;
        };
        match classify(&text) {
            Ok(hook) => hooks.push(hook),
            Err(reason) => gaps.push(format!("`{rel}` was left out of boot: {reason}")),
        }
    }
    if hooks.is_empty() {
        return (None, gaps);
    }
    (Some(render(&hooks)), gaps)
}

/// Warnings for `.rake` files this pass did not install. Not mixins:
/// a Resque file is not an `include` that failed to resolve.
pub(crate) fn gap_diagnostics(app: &App) -> Vec<Diagnostic> {
    app.rake_task_gaps
        .iter()
        .map(|message| {
            let mut d = Diagnostic::unsupported(Span::synthetic(), None, "rake task", message.clone());
            d.severity = Severity::Warning;
            d
        })
        .collect()
}

fn classify(text: &str) -> Result<Hook, String> {
    let parsed = super::prism::parse_silent(text.as_bytes());
    let root = parsed.node();
    let Some(program) = root.as_program_node() else {
        return Err("it is not a Ruby program".into());
    };
    let stmts = super::util::flatten_statements(program.statements().as_node());

    let mut prepended = Vec::new();
    let mut tasks = Vec::new();
    for stmt in &stmts {
        let Some(call) = stmt.as_call_node() else { continue };
        if let Some(name) = prepended_module(&call, text) {
            prepended.push(name);
        }
        if super::util::constant_id_str(&call.name()) == "namespace" {
            if let Some(ns) = first_symbol(&call) {
                tasks.extend(tasks_in(&call, text, &ns));
            }
        }
    }
    let [module_name] = prepended.as_slice() else {
        return Err(outline(&stmts, text));
    };
    let module = stmts.iter().find_map(|stmt| {
        let module = stmt.as_module_node()?;
        let name = super::app::constant_text(&module.constant_path(), text)?;
        (name == *module_name).then_some(module)
    });
    let Some(module) = module else {
        return Err(format!(
            "it prepends `{module_name}`, which this file does not define"
        ));
    };
    let module_source = module_source(&module, text)?;
    if tasks.is_empty() {
        return Err(format!(
            "it prepends `{module_name}` but defines no namespaced Rake task"
        ));
    }
    Ok(Hook { module_name: module_name.clone(), module_source, tasks })
}

fn outline(stmts: &[Node<'_>], src: &str) -> String {
    let mut bits = Vec::new();
    for stmt in stmts {
        if let Some(module) = stmt.as_module_node() {
            if let Some(name) = super::app::constant_text(&module.constant_path(), src) {
                bits.push(format!("module {name}"));
            }
            continue;
        }
        let Some(call) = stmt.as_call_node() else { continue };
        let name = super::util::constant_id_str(&call.name());
        if name == "namespace" {
            match first_symbol(&call) {
                Some(ns) => bits.push(format!("namespace :{ns}")),
                None => bits.push("namespace".into()),
            }
        } else if name == "prepend" {
            bits.push("prepend".into());
        } else if !name.is_empty() {
            bits.push(name.to_string());
        }
    }
    if bits.is_empty() {
        "it does not prepend onto ActiveRecord::Tasks::DatabaseTasks".into()
    } else {
        format!(
            "it has {} and does not prepend onto ActiveRecord::Tasks::DatabaseTasks",
            bits.join(", ")
        )
    }
}

fn prepended_module(call: &ruby_prism::CallNode<'_>, src: &str) -> Option<String> {
    if super::util::constant_id_str(&call.name()) != "prepend" {
        return None;
    }
    let recv = call.receiver()?.as_call_node()?;
    if super::util::constant_id_str(&recv.name()) != "singleton_class" {
        return None;
    }
    let base = recv.receiver()?;
    if super::app::constant_text(&base, src).as_deref()
        != Some("ActiveRecord::Tasks::DatabaseTasks")
    {
        return None;
    }
    let args: Vec<_> = call.arguments()?.arguments().iter().collect();
    let [arg] = args.as_slice() else { return None };
    super::app::constant_text(arg, src)
}

fn module_source(module: &ruby_prism::ModuleNode<'_>, src: &str) -> Result<String, String> {
    let name = super::app::constant_text(&module.constant_path(), src)
        .ok_or_else(|| "its module name is not a constant".to_string())?;
    let body = module.body().ok_or_else(|| format!("module {name} is empty"))?;
    let mut saw_wrapper = false;
    let mut parts = Vec::new();
    for stmt in super::util::flatten_statements(body) {
        if let Some(def) = stmt.as_def_node() {
            let dname = def_name(&def);
            if dname == "load_schema" || dname == "migrate" {
                if !calls_super(&stmt) {
                    return Err(format!("`{dname}` does not call super"));
                }
                saw_wrapper = true;
                if forwards(&def, src) {
                    parts.push(expanded_def(&def, src, &dname));
                } else {
                    parts.push(slice(src, &stmt).to_string());
                }
                continue;
            }
            if forwards(&def, src) {
                return Err(format!(
                    "`{dname}` uses argument forwarding, which this runtime does not rewrite"
                ));
            }
        }
        parts.push(slice(src, &stmt).to_string());
    }
    if !saw_wrapper {
        return Err(format!(
            "module {name} does not wrap load_schema or migrate"
        ));
    }
    let body = parts
        .iter()
        .map(|part| indent(part, 2))
        .collect::<String>();
    Ok(format!("module {name}\n{body}end\n"))
}

fn expanded_def(def: &ruby_prism::DefNode<'_>, src: &str, name: &str) -> String {
    let sig = if name == "load_schema" {
        "def load_schema(db_config, format = \"\", file = \"\")"
    } else {
        "def migrate(version = \"\")"
    };
    let body = def.body().map(|node| slice(src, &node)).unwrap_or("");
    format!("{sig}\n{body}\nend")
}

fn tasks_in(namespace: &ruby_prism::CallNode<'_>, src: &str, ns: &str) -> Vec<Task> {
    let Some(block) = namespace.block().and_then(|node| node.as_block_node()) else {
        return Vec::new();
    };
    let Some(body) = block.body() else { return Vec::new() };
    let mut out = Vec::new();
    for stmt in super::util::flatten_statements(body) {
        let Some(call) = stmt.as_call_node() else { continue };
        if super::util::constant_id_str(&call.name()) != "task" {
            continue;
        }
        let Some((task, prereq)) = task_pair(&call) else { continue };
        let Some(block) = call.block().and_then(|node| node.as_block_node()) else {
            continue;
        };
        let body = block.body().map(|node| slice(src, &node)).unwrap_or("").to_string();
        if body.trim().is_empty() {
            continue;
        }
        out.push(Task { name: format!("{ns}:{task}"), prerequisite: prereq, body });
    }
    out
}

fn task_pair(call: &ruby_prism::CallNode<'_>) -> Option<(String, String)> {
    let args = call.arguments()?;
    for arg in args.arguments().iter() {
        let hash = arg.as_keyword_hash_node()?;
        for element in hash.elements().iter() {
            let pair = element.as_assoc_node()?;
            let name = super::util::symbol_value(&pair.key())?;
            let prereq = super::util::symbol_value(&pair.value())?;
            if ident_method(&name) && ident_method(&prereq) {
                return Some((name, prereq));
            }
        }
    }
    None
}

fn first_symbol(call: &ruby_prism::CallNode<'_>) -> Option<String> {
    let args = call.arguments()?;
    let first = args.arguments().iter().next()?;
    let name = super::util::symbol_value(&first)?;
    ident_method(&name).then_some(name)
}

fn calls_super(node: &Node<'_>) -> bool {
    struct Seen(bool);
    impl<'pr> ruby_prism::Visit<'pr> for Seen {
        fn visit_super_node(&mut self, node: &ruby_prism::SuperNode<'pr>) {
            self.0 = true;
            ruby_prism::visit_super_node(self, node);
        }
        fn visit_forwarding_super_node(&mut self, node: &ruby_prism::ForwardingSuperNode<'pr>) {
            self.0 = true;
            ruby_prism::visit_forwarding_super_node(self, node);
        }
    }
    let mut seen = Seen(false);
    ruby_prism::Visit::visit(&mut seen, node);
    seen.0
}

fn forwards(def: &ruby_prism::DefNode<'_>, src: &str) -> bool {
    let Some(params) = def.parameters() else { return false };
    let loc = params.location();
    src[loc.start_offset()..loc.end_offset()].contains("...")
}

fn def_name(def: &ruby_prism::DefNode<'_>) -> String {
    String::from_utf8_lossy(def.name().as_slice()).into_owned()
}

fn slice<'a>(src: &'a str, node: &Node<'_>) -> &'a str {
    let loc = node.location();
    &src[loc.start_offset()..loc.end_offset()]
}

fn indent(src: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    let mut out = String::new();
    for line in src.lines() {
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            out.push_str(&pad);
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn render(hooks: &[Hook]) -> String {
    let mut out = String::from("# frozen_string_literal: true\n");
    for hook in hooks {
        out.push_str(&hook.module_source);
        out.push_str("\nmodule ActiveRecord\n  module Tasks\n    module DatabaseTasks\n      class << self\n        prepend ");
        out.push_str(&hook.module_name);
        out.push_str("\n      end\n    end\n  end\nend\n\n");
    }
    out.push_str("module AppRakeTasks\n  def self.install!\n");
    for hook in hooks {
        for task in &hook.tasks {
            out.push_str(&format!(
                "    Rake::Task.define(\"{name}\", [\"{pre}\"], \"{name}\")\n",
                name = task.name,
                pre = task.prerequisite
            ));
        }
    }
    out.push_str("    nil\n  end\n\n  def self.run(name)\n    case name\n");
    for hook in hooks {
        for task in &hook.tasks {
            out.push_str(&format!("    when \"{}\"\n", task.name));
            out.push_str(&indent(&task.body, 6));
        }
    }
    out.push_str("    end\n    nil\n  end\nend\n");
    out
}

fn ident_method(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAMPFIRE: &str = r#"
module RoomMessagesCountDatabaseTasks
  def load_schema(...)
    super.tap { repair_room_messages_count }
  end

  def migrate(...)
    super.tap { repair_room_messages_count }
  end

  private
    def repair_room_messages_count
      migration_connection_pool.with_connection do |connection|
        Room::MessagesCount.ensure!(connection)
      end
    end
end

ActiveRecord::Tasks::DatabaseTasks.singleton_class.prepend(RoomMessagesCountDatabaseTasks)

namespace :room_messages_count do
  task ensure: :environment do
    ActiveRecord::Base.connection_pool.with_connection do |connection|
      Room::MessagesCount.ensure!(connection)
    end
  end
end
"#;

    #[test]
    fn campfire_counter_rake_keeps_the_method_bodies() {
        let source = render(&[classify(CAMPFIRE).expect("hook")]);
        assert!(source.contains("class << self\n        prepend RoomMessagesCountDatabaseTasks"));
        assert!(source.contains("super.tap { repair_room_messages_count }"));
        assert!(source.contains("def repair_room_messages_count"));
        assert!(source.contains("migration_connection_pool.with_connection"));
        assert!(source.contains(
            "Rake::Task.define(\"room_messages_count:ensure\", [\"environment\"], \"room_messages_count:ensure\")"
        ));
        assert!(source.contains("def load_schema(db_config, format = \"\", file = \"\")"));
        assert!(!source.contains("..."));
        assert!(!source.contains("singleton_class.prepend"));
        assert!(!source.contains("require \"rake\""));
    }

    #[test]
    fn a_resque_rake_file_is_described_rather_than_called_a_hook() {
        let Err(reason) = classify("namespace :resque do\n  task :setup\nend\n") else {
            panic!("resque file was treated as a hook");
        };
        assert!(reason.contains("namespace :resque"), "{reason}");
        assert!(reason.contains("does not prepend onto ActiveRecord::Tasks::DatabaseTasks"));
    }

    #[test]
    fn a_wrapper_that_does_not_call_super_is_not_rewritten() {
        let src = r#"
module NotAHook
  def load_schema(db_config)
    db_config
  end
end
ActiveRecord::Tasks::DatabaseTasks.singleton_class.prepend(NotAHook)
namespace :widgets do
  task prepare: :environment do
    widgets
  end
end
"#;
        let Err(reason) = classify(src) else {
            panic!("a wrapper that does not call super was treated as a hook");
        };
        assert!(reason.contains("`load_schema` does not call super"), "{reason}");
    }
}
