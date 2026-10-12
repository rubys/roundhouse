//! A class's own `then` answers its declaration, not Kernel's.
//!
//! Kernel#then hands back the block's value. A promise's `then` yields
//! the fulfilled value and answers a new promise, so the block's
//! parameters stay unbound for it, and the Kernel answer (the block's
//! value, typed from an unbound parameter) reported
//! `load_client.then { |client| … }` as `no known method then on
//! Promise`.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn check(name: &str, account: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "rh_own_then_dispatch_{}_{name}_{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let files = [
        ("roundhouse.yml", "test_paths:\n  - test\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :accounts do |t|\n    t.string :name\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/services/deferred.rb", "class Deferred\n  #: () { (untyped) -> untyped } -> Deferred\n  def then(&block) = self\nend\n"),
        ("app/models/account.rb", account),
    ];
    for (path, text) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(&root).output().unwrap();
    let _ = std::fs::remove_dir_all(&root);
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn the_owner_answers_and_kernel_then_still_hands_back_the_block() {
    let account = "class Account < ApplicationRecord\n  #: -> untyped\n  def use = [Deferred.new.then { |v| v }.zzz, 1.then { |x| x.to_s }.yyy]\nend\n";
    let text = check("instance", account);
    assert!(!text.contains("no known method `then`"), "{text}");
    assert!(text.contains("no known method `zzz` on Deferred"), "{text}");
    assert!(text.contains("no known method `yyy` on String"), "{text}");
}

#[test]
fn a_class_side_then_answers_its_declaration_not_kernels() {
    let account = "class Account < ApplicationRecord\n  #: () { (untyped) -> untyped } -> Account\n  def self.then(&block) = self\n  #: -> untyped\n  def use = [Account.then { |v| v }.zzz, \"x\".then { |s| s.upcase }.yyy]\nend\n";
    let text = check("class", account);
    assert!(!text.contains("no known method `then`"), "{text}");
    assert!(text.contains("no known method `zzz` on Account"), "{text}");
    assert!(text.contains("no known method `yyy` on String"), "{text}");
}
