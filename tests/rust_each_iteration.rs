//! Rust `.each` emission borrows elements immutably unless the block
//! actually invokes a mutating method on its yielded element.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted_view(template: &str) -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            b"ActiveRecord::Schema.define do\n  create_table \"subscriptions\", force: :cascade do |t|\n    t.string \"endpoint\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            "app/models/subscription.rb",
            b"class Subscription < ApplicationRecord\nend\n".to_vec(),
        ),
        (
            "app/controllers/users_controller.rb",
            b"class UsersController < ApplicationController\n  def index\n    @push_subscriptions = Subscription.all\n  end\nend\n".to_vec(),
        ),
        (
            "app/views/users/index.html.erb",
            template.as_bytes().to_vec(),
        ),
    ]
    .into_iter()
    .map(|(path, content)| (PathBuf::from(path), content))
    .collect();
    let app = ingest_app_from_tree(files).expect("ingest tree");
    rust::emit(&app)
        .into_iter()
        .find(|file| {
            file.path.to_string_lossy().starts_with("src/views/")
                && file.content.contains("fn index")
        })
        .unwrap_or_else(|| panic!("missing emitted users/index view"))
        .content
}

#[test]
fn read_only_view_each_uses_immutable_iteration() {
    let view = emitted_view("<% [1, 2].each do |subscription| %><%= subscription %><% end %>\n");
    assert!(
        view.contains(".iter().for_each("),
        "expected `.iter()`:\n{view}"
    );
    assert!(
        !view.contains(".iter_mut()"),
        "a read-only view loop must not require a mutable binding:\n{view}"
    );
}

#[test]
fn each_with_nonlocal_return_emits_a_loop_instead_of_a_closure() {
    let files: HashMap<PathBuf, Vec<u8>> = [(
        "app/models/iteration_probe.rb",
        b"class IterationProbe\n  def self.any_match\n    [1, 2].each do |item|\n      return true if item == 2\n    end\n    false\n  end\n\n  def self.find_match\n    [1, 2].each do |item|\n      return item if item == 2\n    end\n    nil\n  end\nend\n".to_vec(),
    )]
    .into_iter()
    .map(|(path, content)| (PathBuf::from(path), content))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let source = rust::emit(&app)
        .into_iter()
        .find(|file| file.content.contains("fn any_match"))
        .expect("IterationProbe method")
        .content;

    assert!(
        source.contains("for item in vec![1_i64, 2_i64].iter()"),
        "non-local returns require a native loop:\n{source}"
    );
    assert!(
        source.contains("return true"),
        "the return must target the enclosing method:\n{source}"
    );
    assert!(
        !source.contains("for_each(|item|"),
        "do not put Ruby non-local returns inside a Rust closure:\n{source}"
    );
    assert!(
        source.contains("return Some(item)"),
        "an optional enclosing method must wrap a found loop item in Some:\n{source}"
    );
}

#[test]
fn custom_empty_predicate_calls_use_the_emitted_method_name() {
    let tree = [(
        PathBuf::from("app/models/predicate_probe.rb"),
        b"class PredicateProbe\n  def empty?\n    true\n  end\n\n  def check_empty\n    empty?\n  end\n\n  def self.empty_collection\n    [].empty?\n  end\nend\n".to_vec(),
    )]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest synthetic app");
    roundhouse::session::analyze_and_lower(&mut app);
    let source = rust::emit(&app)
        .into_iter()
        .find(|file| file.content.contains("fn check_empty"))
        .expect("PredicateProbe method")
        .content;

    assert!(
        source.contains("self.empty_pred()"),
        "the custom `empty?` call must target its emitted predicate method:\n{source}"
    );
    assert!(
        !source.contains("self.is_empty()"),
        "a custom predicate must not be mistaken for a Rust collection method:\n{source}"
    );
    assert!(
        source.contains(".is_empty()"),
        "the typed collection bridge must still use Rust's `is_empty` method:\n{source}"
    );
}
