//! `roundhouse check` counts the sends nothing checked: an explicit
//! receiver of unknown or gradual type consults no method table, so a
//! missing method passes. In a library class nothing else reports them.
//! The count sits beside the error total so that an error that goes
//! away by becoming such a send is visible as a reclassification.
use std::process::Command;

fn check(args: &[&str], files: &[(&str, &str)]) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("rh_unknown_sends_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n"),
    ].iter().chain(files) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").args(args).arg(&root).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

const LIB: &str = "class Lookup\n  def has?(value)\n    value.include?(1)\n  end\n\n  def loose(value)\n    T.unsafe(value).frobnicate\n  end\n\n  def known\n    [1].include?(1)\n  end\n\n  def missing\n    [1].frobnicate\n  end\nend\n";

#[test]
fn library_sends_on_unknown_and_gradual_receivers_are_counted() {
    let text = check(&[], &[("app/lib/lookup.rb", LIB)]);
    assert!(
        text.contains("roundhouse-check: 1 send(s) on unknown receivers, 1 on gradual receivers, 1 unresolved on known receivers"),
        "{text}"
    );
}

#[test]
fn each_unchecked_send_can_be_listed() {
    let text = check(&["--unknown-sends"], &[("app/lib/lookup.rb", LIB)]);
    assert!(text.contains("send `include?` on an unknown receiver is unchecked"), "{text}");
    assert!(text.contains("send `frobnicate` on a gradual (untyped) receiver is unchecked"), "{text}");
    assert!(text.contains("send `frobnicate` resolves to no method on its known receiver and is unchecked"), "{text}");
    // A typed receiver whose send resolves is checked, so it is not listed.
    assert_eq!(text.matches("is unchecked").count(), 3, "{text}");
}

/// In a model body the same unresolved send is already an error
/// (`send_dispatch_failed`), so it is not counted a second time.
#[test]
fn a_diagnosed_unresolved_send_is_not_also_counted() {
    let model = "class Probe < ApplicationRecord\n  def missing\n    [1].frobnicate\n  end\nend\n";
    let text = check(&[], &[("app/models/probe.rb", model)]);
    assert!(text.contains("no known method `frobnicate`"), "{text}");
    assert!(text.contains(", 0 unresolved on known receivers"), "{text}");
}

/// A receiver whose type names a class the registry does not hold has no
/// method table to answer for it, so its sends are unknown-receiver sends.
#[test]
fn a_receiver_typed_as_an_unregistered_class_is_unknown() {
    let lib = "class Syncer\n  #: (Missing::AccessControl) -> bool\n  def has(access)\n    access.lookup(\"a\")\n  end\n\n  #: (Missing::AccessControl) -> bool\n  def other(access)\n    access.frobnicate(\"a\")\n  end\nend\n";
    let text = check(&[], &[("app/lib/syncer.rb", lib)]);
    assert!(
        text.contains("roundhouse-check: 2 send(s) on unknown receivers, 0 on gradual receivers, 0 unresolved on known receivers"),
        "{text}"
    );
}
