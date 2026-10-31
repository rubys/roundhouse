//! PostgreSQL: class body self writers, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

use std::path::PathBuf;

use std::collections::HashMap;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;

const PG_FILES: [(&str, &str); 4] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", "default: &default\n  adapter: postgresql\n\ntest:\n  <<: *default\n  database: x\n"),
    ("db/structure.sql", "CREATE TABLE items (\n    id bigserial NOT NULL,\n    CONSTRAINT items_pkey PRIMARY KEY (id)\n);\n"),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
];

/// The Ruby tree's files and the diagnostics the emit reported.
fn emitted(files: &[(&str, &str)]) -> (Vec<(String, String)>, Vec<String>) {
    let tree: HashMap<PathBuf, Vec<u8>> = PG_FILES
        .iter()
        .chain(files)
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let (out, diags) = roundhouse::emit::diagnostics::scope(|| {
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        roundhouse::session::analyze_and_lower(&mut app);
        roundhouse::project::target_files(&app, std::path::Path::new("not-a-fixture"), BuildTarget::Ruby)
    });
    (out.expect("ruby files"), diags.iter().map(|d| d.message.clone()).collect())
}

fn file<'a>(files: &'a [(String, String)], name: &str) -> &'a str {
    &files.iter().find(|(p, _)| p.ends_with(name)).unwrap_or_else(|| panic!("{name} not emitted")).1
}

/// The text of `body` from `from` on.
fn lines_after<'a>(body: &'a str, from: &str) -> &'a str {
    &body[body.find(from).unwrap_or_else(|| panic!("`{from}` not in:\n{body}"))..]
}

fn dropped(diags: &[String], writer: &str) -> bool {
    diags.iter().any(|m| m.contains(&format!("`{writer}`")) && m.contains("dropped"))
}

const RECORDER: &str = r#"class MethodTimingRecorder < Module
  attr_reader :time

  class << self
    attr_accessor :instances

    def reset_all_instances
      instances.each(&:reset)
    end
  end

  self.instances = []

  def initialize(method_name, &block)
    super(&block)
    @time = nil
    self.class.instances.push(self)
  end
  private :initialize

  def reset
    @time = nil
  end

  private

  module CallMethod
    private

    def _call_original_method(callable, recorder)
      callable.call
    end
  end

  class MethodNotCalled < StandardError
  end
end
"#;

#[test]
fn a_writer_from_the_class_side_attr_accessor_is_carried() {
    let (files, diags) = emitted(&[("app/models/method_timing_recorder.rb", RECORDER)]);
    let src = file(&files, "method_timing_recorder.rb");
    assert!(!dropped(&diags, "instances="), "{diags:#?}");
    // After the writer's own definition, which it calls.
    let after_writer = lines_after(src, "def self.instances=(value)");
    assert!(after_writer.contains("\n  self.instances = []\n"), "{src}");
}

const BASE_TASK: &str = "module MaintenanceTasks\n  class Task\n    def collection\n      []\n    end\n  end\nend\n";

const TASK_INITIALIZER: &str = r#"Rails.autoloaders.main.on_load("MaintenanceTasks::Task") do
  MaintenanceTasks::Task.class_attribute(:unscoped_shop, default: false, instance_writer: false)
end
"#;

const SHOP_TASK: &str = r#"module Maintenance
  class UnscopedShopTask < MaintenanceTasks::Task
    self.unscoped_shop = true

    def process(shop)
      shop
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_writer_an_initializer_declares_on_the_base_is_carried() {
    let (files, diags) = emitted(&[
        ("app/models/maintenance_tasks/task.rb", BASE_TASK),
        ("config/initializers/maintenance_tasks.rb", TASK_INITIALIZER),
        ("app/tasks/maintenance/unscoped_shop_task.rb", SHOP_TASK),
    ]);
    let src = file(&files, "unscoped_shop_task.rb");
    assert!(!dropped(&diags, "unscoped_shop="), "{diags:#?}");
    assert!(src.contains("self.unscoped_shop = true"), "{src}");
}

#[test]
fn a_writer_nothing_declares_stays_reported_as_dropped() {
    let (_, diags) = emitted(&[
        ("app/models/maintenance_tasks/task.rb", BASE_TASK),
        ("app/tasks/maintenance/unscoped_shop_task.rb", SHOP_TASK),
    ]);
    assert!(dropped(&diags, "unscoped_shop="), "{diags:#?}");
}

#[test]
fn writers_of_the_class_attribute_cattr_and_def_forms_are_carried() {
    let (files, diags) = emitted(&[
        ("app/models/base_task.rb", "class BaseTask\n  class_attribute :label\nend\n"),
        ("app/models/shop_task.rb", "class ShopTask < BaseTask\n  self.label = \"x\"\n\n  def run\n    1\n  end\nend\n"),
        ("app/models/own_cattr.rb", "class OwnCattr\n  cattr_accessor :level\n  self.level = 3\n\n  def run\n    level\n  end\nend\n"),
        (
            "app/models/own_def.rb",
            "class OwnDef\n  def self.mode=(v)\n    @mode = v\n  end\n\n  self.mode = :fast\n\n  def run\n    1\n  end\nend\n",
        ),
    ]);
    assert!(file(&files, "shop_task.rb").contains("self.label = \"x\""));
    assert!(file(&files, "own_cattr.rb").contains("self.level = 3"));
    assert!(file(&files, "own_def.rb").contains("self.mode = :fast"));
    for writer in ["label=", "level=", "mode="] {
        assert!(!dropped(&diags, writer), "{writer}: {diags:#?}");
    }
}
