//! `lib/tasks/*.rake` that prepend a hook onto
//! `ActiveRecord::Tasks::DatabaseTasks`.
//!
//! Campfire's `room_messages_count.rake` is the shape: a module of
//! `load_schema` / `migrate` wrappers, prepended onto the singleton,
//! plus one namespaced task. The generated file is required from the
//! shared runtime so the prepend is in the ancestor chain at boot on
//! every Ruby-family target, including Spinel. Anything else in that
//! directory is named and left out — a warning, not a pretend task.

use std::path::Path;

use crate::vfs::Vfs;

struct Hook {
    module_name: String,
    wraps_load_schema: bool,
    wraps_migrate: bool,
    repair: String,
    ensure_receiver: String,
    task_name: Option<String>,
    task_prerequisite: Option<String>,
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
            gaps.push(rel);
            continue;
        };
        match parse(&text) {
            Some(hook) => hooks.push(hook),
            None => gaps.push(rel),
        }
    }
    if hooks.is_empty() {
        return (None, gaps);
    }
    (Some(render(&hooks)), gaps)
}

fn parse(text: &str) -> Option<Hook> {
    if !text.contains("singleton_class.prepend") && !text.contains("singleton_class.prepend(") {
        return None;
    }
    let module_name = capture_module(text)?;
    if !ident(&module_name) {
        return None;
    }
    let wraps_load_schema = text.contains("def load_schema");
    let wraps_migrate = text.contains("def migrate");
    if !wraps_load_schema && !wraps_migrate {
        return None;
    }
    let repair = capture_def_name(text, "repair")?;
    let ensure_receiver = capture_ensure(text)?;
    if !const_path(&ensure_receiver) {
        return None;
    }
    let (task_name, task_prerequisite) = capture_task(text);
    Some(Hook {
        module_name,
        wraps_load_schema,
        wraps_migrate,
        repair,
        ensure_receiver,
        task_name,
        task_prerequisite,
    })
}

fn capture_module(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("module ") else { continue };
        let Some(name) = rest.split_whitespace().next() else { continue };
        if ident(name) {
            return Some(name.to_string());
        }
    }
    None
}

fn capture_def_name(text: &str, prefix: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("def ") else { continue };
        let Some(name) = rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next() else {
            continue;
        };
        if name.starts_with(prefix) && ident_method(name) {
            return Some(name.to_string());
        }
    }
    None
}

fn capture_ensure(text: &str) -> Option<String> {
    let at = text.find(".ensure!")?;
    let bytes = text.as_bytes();
    let mut start = at;
    while start > 0 {
        let ch = bytes[start - 1] as char;
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == ':' {
            start -= 1;
        } else {
            break;
        }
    }
    let name = &text[start..at];
    let name = name.trim_start_matches(':');
    if const_path(name) { Some(name.to_string()) } else { None }
}

fn capture_task(text: &str) -> (Option<String>, Option<String>) {
    let mut namespace = None;
    let mut task = None;
    let mut prereq = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("namespace :") {
            if let Some(name) = rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next() {
                if !name.is_empty() {
                    namespace = Some(name.to_string());
                }
            }
        }
        if let Some(rest) = line.strip_prefix("task ") {
            // `task ensure: :environment` or `task ensure: :environment do`
            let rest = rest.trim();
            let Some((name, after)) = rest.split_once(':') else { continue };
            let name = name.trim().trim_start_matches(':');
            let after = after.trim().trim_start_matches(':');
            let pre = after.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next().unwrap_or("");
            if ident_method(name) && ident_method(pre) {
                task = Some(name.to_string());
                prereq = Some(pre.to_string());
            }
        }
    }
    let full = match (namespace, task) {
        (Some(ns), Some(task)) => Some(format!("{ns}:{task}")),
        _ => None,
    };
    (full, prereq)
}

fn render(hooks: &[Hook]) -> String {
    let mut out = String::from("# frozen_string_literal: true\n");
    for hook in hooks {
        out.push_str(&render_hook(hook));
    }
    out.push_str("module AppRakeTasks\n");
    out.push_str("  def self.install!\n");
    for hook in hooks {
        if let (Some(name), Some(pre)) = (&hook.task_name, &hook.task_prerequisite) {
            out.push_str(&format!(
                "    Rake::Task.define(\"{name}\", [\"{pre}\"], \"{name}\")\n"
            ));
        }
    }
    out.push_str("    nil\n  end\n\n");
    out.push_str("  def self.run(name)\n    case name\n");
    for hook in hooks {
        if let Some(name) = &hook.task_name {
            out.push_str(&format!(
                "    when \"{name}\"\n      ActiveRecord::Base.connection_pool.with_connection do |connection|\n        {recv}.ensure!(connection)\n      end\n",
                recv = hook.ensure_receiver
            ));
        }
    }
    out.push_str("    end\n    nil\n  end\nend\n");
    out
}

fn render_hook(hook: &Hook) -> String {
    let mut body = format!("module {}\n", hook.module_name);
    if hook.wraps_load_schema {
        body.push_str(&format!(
            "  def load_schema(db_config, format = \"\", file = \"\")\n    result = super(db_config, format, file)\n    {repair}\n    result\n  end\n\n",
            repair = hook.repair
        ));
    }
    if hook.wraps_migrate {
        body.push_str(&format!(
            "  def migrate(version = \"\")\n    result = super(version)\n    {repair}\n    result\n  end\n\n",
            repair = hook.repair
        ));
    }
    body.push_str(&format!(
        "  def {repair}\n    migration_connection_pool.with_connection do |connection|\n      {recv}.ensure!(connection)\n    end\n  end\nend\n\n",
        repair = hook.repair,
        recv = hook.ensure_receiver
    ));
    body.push_str("module ActiveRecord\n  module Tasks\n    module DatabaseTasks\n      class << self\n        prepend ");
    body.push_str(&hook.module_name);
    body.push_str("\n      end\n    end\n  end\nend\n\n");
    body
}

fn ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => chars.all(|c| c.is_ascii_alphanumeric() || c == '_'),
        _ => false,
    }
}

fn ident_method(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => chars.all(|c| c.is_ascii_alphanumeric() || c == '_'),
        _ => false,
    }
}

fn const_path(name: &str) -> bool {
    !name.is_empty() && name.split("::").all(ident)
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
    fn campfire_counter_rake_becomes_a_singleton_prepend() {
        let source = render(&[parse(CAMPFIRE).expect("hook")]);
        assert!(source.contains("class << self\n        prepend RoomMessagesCountDatabaseTasks"));
        assert!(source.contains("Room::MessagesCount.ensure!(connection)"));
        assert!(source.contains("Rake::Task.define(\"room_messages_count:ensure\", [\"environment\"], \"room_messages_count:ensure\")"));
        assert!(!source.contains("..."));
        assert!(!source.contains("require \"rake\""));
    }

    #[test]
    fn a_resque_rake_file_is_not_a_database_hook() {
        assert!(parse("namespace :resque do\n  task :setup\nend\n").is_none());
    }
}
