//! An app class named `Base`, and a class extending it, in an app whose
//! models and controllers extend the runtime's `ActiveRecord::Base` and
//! `ActionController::Base` (#1228).

use super::emit_and_run::{self, Overlay};

pub const BASE: &str = "class Base\n  LABEL = \"base\"\n\n  def self.label\n    LABEL\n  end\nend\n";
pub const WIDGET: &str = "class Widget < Base\nend\n";

/// What `[Widget.label, Widget::LABEL].join(",")` answers under CRuby.
#[allow(dead_code)]
pub const SUMMARY: &str = "base,base";

pub fn overlay() -> Overlay {
    emit_and_run::real_blog().write("app/models/base.rb", BASE).write("app/models/widget.rb", WIDGET)
}

/// The app also defines a class named after the alias the runtime
/// record base would take, and it extends that runtime base.
pub const ACTIVE_RECORD_BASE: &str =
    "class ActiveRecordBase < ActiveRecord::Base\n  self.abstract_class = true\n\n  def self.tag\n    \"arb\"\n  end\nend\n";
pub const GADGET: &str =
    "class Gadget < ApplicationRecord\n  def self.both\n    [ActiveRecordBase.tag, Base.label].join(\",\")\n  end\nend\n";

/// What `Gadget.both` answers under CRuby.
#[allow(dead_code)]
pub const BOTH: &str = "arb,base";

pub fn alias_taken_overlay() -> Overlay {
    overlay().write("app/models/active_record_base.rb", ACTIVE_RECORD_BASE).write("app/models/gadget.rb", GADGET)
}

