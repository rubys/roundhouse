//! `include ActiveSupport::Concern`, written where the idiom is
//! `extend`, must not read as the module including an unresolved
//! external class. On Rails this slip is a dead no-op: `Module#include`
//! runs Concern's own `append_features`/`included`/`class_methods` as
//! unused instance methods, nothing calls them that way, and the
//! module's real methods still reach every includer.

use std::process::Command;

fn check(files: &[(&str, &str)]) -> String {
    let dir = std::env::temp_dir().join(format!("rh_concern_marker_include_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, src) in files {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, src).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["check", "--continue"])
        .arg(&dir)
        .output()
        .expect("spawn roundhouse");
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";
const TAGGABLE: &str =
    "module Taggable\n  include ActiveSupport::Concern\n\n  def label\n    \"tagged\"\n  end\nend\n";
const WIDGET: &str =
    "class Widget < ApplicationRecord\n  include Taggable\n\n  def show_label\n    label.upcase\n  end\nend\n";
const GADGETS_CONTROLLER: &str =
    "class GadgetsController < ActionController::Base\n  include Taggable\n\n  def index\n    render plain: label\n  end\nend\n";
const ROUTES: &str = "Rails.application.routes.draw do\n  get \"gadgets\", to: \"gadgets#index\"\nend\n";

#[test]
fn include_active_support_concern_is_a_dead_marker_not_an_unresolved_include() {
    let err = check(&[
        ("app/models/application_record.rb", RECORD),
        ("app/models/concerns/taggable.rb", TAGGABLE),
        ("app/models/widget.rb", WIDGET),
        ("app/controllers/gadgets_controller.rb", GADGETS_CONTROLLER),
        ("config/routes.rb", ROUTES),
    ]);
    assert!(
        !err.contains("includes unresolved"),
        "include ActiveSupport::Concern must not be treated as an unresolved include: {err}"
    );
    // Both the model's and the controller's calls to `label` must
    // resolve through the include — had the module's method surface
    // been damaged, each would show up as "no known method `label`".
    assert!(!err.contains("`label`"), "the concern's method must still reach every includer: {err}");
    assert!(err.contains("0 error(s)"), "{err}");
}
