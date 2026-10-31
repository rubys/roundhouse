//! A by-name `StringInquirer` fact must not fold a receiver-less predicate
//! in a class that cannot reach the method it was collected from.
//!
//! `ActiveSupport::StringInquirer` turns `x.<label>?` into `x == "<label>"`
//! (activesupport/lib/active_support/string_inquirer.rb), so the lowering
//! folds a predicate whose receiver is a call to a method that returns an
//! inquirer. The method set is keyed by NAME, and a receiver-less `kind`
//! dispatches on the owner's own `self`: in a mixin that defines no `kind`
//! it is the host's reader (a GraphQL type's `TypeKind#interface?`), not
//! some other class's inquirer-returning `kind`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(file: &str) -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"clients\", force: :cascade do |t|\n    t.string \"kind\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/client.rb"),
            b"class Client < ApplicationRecord\n  def kind\n    ActiveSupport::StringInquirer.new(self[:kind].to_s)\n  end\n\n  def internal?\n    kind.internal?\n  end\n\n  def self.internal_kind?\n    Client.new.kind.internal?\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/typed_member.rb"),
            b"module TypedMember\n  def interface_member?\n    kind.interface?\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/typed_member_probe.rb"),
            b"class TypedMemberProbe\n  def abstract_any?(types)\n    types.any? { |type| type.kind.abstract? }\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/typed_member_host.rb"),
            b"class TypedMemberHost < Client\n  def host_internal?\n    kind.internal?\n  end\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let mut files = ruby::emit_lowered_models(&app);
    files.extend(ruby::emit_library(&app));
    files
        .iter()
        .find(|f| f.path.ends_with(file))
        .unwrap_or_else(|| panic!("no {file} emitted"))
        .content
        .clone()
}

#[test]
fn a_receiverless_predicate_in_an_unrelated_mixin_is_left_alone() {
    let src = emitted("typed_member.rb");
    assert!(src.contains("kind.interface?"), "{src}");
    assert!(!src.contains("kind == \"interface\""), "{src}");
}

#[test]
fn a_receiverless_predicate_folds_in_the_class_that_defines_the_inquirer() {
    let src = emitted("client.rb");
    assert!(src.contains("kind == \"internal\""), "{src}");
    assert!(!src.contains(".internal?"), "{src}");
}

#[test]
fn a_receiverless_predicate_folds_in_a_subclass_of_the_definer() {
    let src = emitted("typed_member_host.rb");
    assert!(src.contains("kind == \"internal\""), "{src}");
}

#[test]
fn an_explicit_receiver_of_unknown_class_is_left_alone() {
    let src = emitted("typed_member_probe.rb");
    assert!(src.contains("type.kind.abstract?"), "{src}");
    assert!(!src.contains("kind == \"abstract\""), "{src}");
}

#[test]
fn an_explicit_receiver_typed_as_the_definer_folds() {
    let src = emitted("client.rb");
    assert!(src.contains("Client.new.kind == \"internal\""), "{src}");
}
