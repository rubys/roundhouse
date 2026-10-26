//! `include` / `prepend` / `extend` that a method runs mix a module into
//! a class while the program runs. A class body's mixin is read at
//! ingest and becomes the emitted class's ancestry; one inside a method
//! (liquid's `StrainerTemplate.add_filter(filter)` → `include(filter)`)
//! passed `check` clean and came out as a call no compiled target has —
//! TypeScript wrote `this.include(filter)`. It is refused at the call.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

fn refusals(strainer: &str) -> Vec<String> {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        ("app/models/up_filter.rb", "module UpFilter\n  def up(s) = s.upcase\nend\n"),
        ("app/models/strainer.rb", strainer),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    diagnose(&app)
        .iter()
        .map(|d| d.to_string())
        .filter(|d| d.contains("Module mixin at run time"))
        .collect()
}

#[test]
fn a_mixin_a_method_runs_is_refused_at_the_call() {
    let found = refusals(
        "class Strainer\n  \
           def self.add_filter(filter)\n    include(filter)\n  end\n\n  \
           def self.decorate\n    extend UpFilter\n  end\n\n  \
           def self.wrap\n    Strainer.prepend(UpFilter)\n  end\n\n  \
           def self.aliased\n    target = Strainer\n    target.include(UpFilter)\n  end\n\n  \
           def self.inherited(base)\n    Strainer.extend(UpFilter)\n  end\n\n  \
           def self.anonymous\n    Class.new { Strainer.include(UpFilter) }\n  end\n\
         end\n",
    );
    for (method, count) in [("`include`", 3), ("`extend`", 2), ("`prepend`", 1)] {
        assert_eq!(found.iter().filter(|d| d.contains(method)).count(), count, "{method} {count} times in {found:?}");
    }
}

#[test]
fn a_class_body_mixin_a_hook_a_built_class_and_a_class_own_include_stay_clean() {
    let found = refusals(
        "class Strainer\n  include UpFilter\n\n  \
           def self.include(name) = name.to_s\n\n  \
           def self.add_filter(filter)\n    include(filter)\n  end\n\n  \
           def self.inherited(base)\n    base.include(UpFilter)\n  end\n\n  \
           def self.build\n    klass = Class.new\n    klass.include(UpFilter)\n    klass\n  end\n\n  \
           def self.[](name)\n    Module.new do\n      extend ActiveSupport::Concern\n      include UpFilter\n    end\n  end\n\
         end\n",
    );
    assert!(found.is_empty(), "{found:?}");
}
