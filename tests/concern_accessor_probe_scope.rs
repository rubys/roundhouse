//! The concern accessor surface probe analyzes and lowers a clone of the
//! whole app. It answers only the last operand of the refusal check
//! ("is the name already defined by the analyzed model?"), so a model whose
//! carried `attr_accessor` is refused by a syntactic check alone must not
//! start it. `ROUNDHOUSE_TIMINGS=1` prints one line per phase, which makes
//! the probe observable as the `concern-accessor-surface` phase.

use std::process::Command;

const CONCERN: &str = "module Virtual\n  extend ActiveSupport::Concern\n  included { attr_accessor :scratch }\nend\n";

fn check(name: &str, widget_body: &str) -> String {
    let root = std::env::temp_dir().join(format!("rh_accessor_probe_scope_{}_{name}", std::process::id()));
    let widget = format!("class Widget < ApplicationRecord\n  include Virtual\n{widget_body}end\n");
    let files = [
        ("roundhouse.yml", "test_paths:\n  - test\n".to_string()),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :title\n  end\nend\n".to_string()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string()),
        ("app/models/concerns/virtual.rb", CONCERN.to_string()),
        ("app/models/widget.rb", widget),
    ];
    for (path, text) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .arg("check")
        .arg(&root)
        .env("ROUNDHOUSE_TIMINGS", "1")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

/// Control: a name that passes the syntactic checks reaches the probe.
#[test]
fn a_surviving_name_is_probed() {
    let text = check("surviving", "");
    assert!(text.contains("concern-accessor-surface"), "{text}");
}

/// A name refused for a private same-named method never reaches it.
#[test]
fn a_syntactically_refused_name_is_not_probed() {
    let text = check("refused", "  private\n\n  def scratch = 1\n");
    assert!(text.contains("concern attr_accessor :scratch"), "{text}");
    assert!(!text.contains("concern-accessor-surface"), "{text}");
}
