//! A helper's NAMED keyword, passed as a keyword, bound to the wrong
//! thing.
//!
//! Ingest lowers an optional keyword parameter to a
//! positional-with-default; the call sites kept the keyword, so Ruby
//! bound the trailing `{for_user: nil}` HASH to the positional
//! `for_user`. campfire's sidebar then reached
//! `room.users.without(for_user)` and emitted
//! `NOT (id.for_user IS NULL)` — SQL naming a column that does not
//! exist.
//!
//! The rule is NAMES: a trailing-kwarg key that matches a positional
//! parameter name means that parameter. Keys that match nothing —
//! `id:`/`class:` against a `**attributes` helper — mean a hash, and
//! are left alone by construction.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :rooms do |t|\n    t.string :name\n  end\nend\n";

fn emit(helper: &str, view: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/rooms_controller.rb"),
        b"class RoomsController < ApplicationController\n  def index\n    @rooms = Room.all\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/helpers/rooms_helper.rb"), helper.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/views/rooms/index.html.erb"), view.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // The VIEW is where the call site lives; the helper module emits
    // separately and would show only the definition.
    ruby::emit_lowered_views(&app)
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

const NAMED_KWARG: &str =
    "module RoomsHelper\n  def label_for(room, upcase: false)\n    upcase ? room.name.upcase : room.name\n  end\nend\n";

/// Native keywords keep their binding instead of becoming a positional Hash.
#[test]
fn a_named_keyword_keeps_its_keyword_slot() {
    let src = emit(NAMED_KWARG, "<%= label_for(@rooms.first, upcase: true) %>\n");
    assert!(src.contains("upcase: true"), "the keyword must retain its source binding:\n{src}");
}

/// A call that already passes it positionally is untouched.
#[test]
fn an_already_positional_call_is_unchanged() {
    let src = emit(NAMED_KWARG, "<%= label_for(@rooms.first, true) %>\n");
    assert!(src.contains(", true)"), "{src}");
}

/// A `**attributes` helper keeps its hash: `id:` matches no parameter
/// name, so the trailing keywords still mean one Hash argument. This is
/// the case the fix must NOT break — it is why the rule is names rather
/// than "move every trailing keyword".
#[test]
fn a_splat_helper_keeps_its_hash() {
    let splat = "module RoomsHelper\n  def wrap(room, **attributes)\n    attributes.merge(name: room.name)\n  end\nend\n";
    let src = emit(splat, "<%= wrap(@rooms.first, id: \"x\") %>\n");
    assert!(
        src.contains("id:"),
        "the trailing keywords must stay a hash:\n{src}"
    );
}

/// THE SAME BREAK INSIDE A TEST CLASS, where the callee is the class's
/// own private helper rather than a helper module's method.
///
/// campfire's `unfurl_links_controller_test` writes
/// `stub_successful_request(url: "https://fxtwitter.com/…")` against
/// `def stub_successful_request(url: "https://www.example.com/")`, and
/// handed WebMock the HASH: "URI should be a String … Got: Hash".
#[test]
fn a_test_classs_own_helper_gets_the_same_repair() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("test/models/room_test.rb"),
        br#"require "test_helper"

class RoomTest < ActiveSupport::TestCase
  test "named" do
    assert_equal "HQ", titled(name: "HQ")
  end

  test "default" do
    assert_equal "Room", titled
  end

  private
    def titled(name: "Room")
      name
    end
end
"#
        .to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // `emit_spinel` is the shared Rails-shape path; the emitted test
    // files ride out with it.
    let src = ruby::emit_spinel(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains("room_test"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains(r#"titled(name: "HQ")"#),
        "the test helper keeps its native keyword spelling:\n{src}"
    );
}

/// A key naming no parameter at all declines — the call means something
/// this pass cannot prove.
#[test]
fn an_unknown_key_declines() {
    let src = emit(NAMED_KWARG, "<%= label_for(@rooms.first, bogus: 1) %>\n");
    assert!(src.contains("bogus:"), "left alone:\n{src}");
}

/// A caller may name the later keywords and leave an earlier one to its
/// default: the skipped slot takes the parameter's own default, the
/// value Ruby binds. campfire's `stub_successful_request(title:,
/// description:)` is the shape.
#[test]
fn a_skipped_keyword_takes_its_own_default() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("test/models/room_test.rb"),
        br##"require "test_helper"

class RoomTest < ActiveSupport::TestCase
  test "later keywords only" do
    assert_equal "https://a.example/ T D", stubbed(title: "T", description: "D")
  end

  test "context-bound default declines" do
    assert_equal "x", relative(b: "x")
  end

  private
    def stubbed(url: "https://a.example/", title: "Hey!", description: "desc..")
      "#{url} #{title} #{description}"
    end

    def relative(a = Room.count, b: "y")
      "#{a}#{b}"
    end
end
"##
        .to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_spinel(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains("room_test"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains(r#"stubbed(title: "T", description: "D")"#),
        "the skipped `url:` must default in the callee, not at the call site:\n{src}"
    );
    assert!(
        src.contains("relative(b: \"x\")"),
        "a default that reads the callee's context is not moved to the call site:\n{src}"
    );
}

fn emit_models(files: &[(&str, &str)], keep: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    for (path, src) in files {
        tree.insert(PathBuf::from(path), src.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains(keep))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!src.is_empty(), "harness emitted no {keep} — every assertion below would be vacuous");
    src
}

/// campfire's `Opengraph::Fetch.new.fetch_content_type(parsed_url, ip:
/// resolved_ip)`, reduced: an INSTANCE method's flattened keyword,
/// passed by name on a receiver typed as the class. The Hash used to
/// bind to `ip` whole, and the unfurl connected to `{ip: "…"}`.
#[test]
fn an_instance_methods_keyword_keeps_its_keyword_slot() {
    let src = emit_models(
        &[
            (
                "app/models/fetcher.rb",
                "class Fetcher\n  def fetch(url, ip: url.upcase)\n    \"#{url}@#{ip}\"\n  end\n\n  \
                 def merge(url, opts = {})\n    \"#{url}#{opts}\"\n  end\nend\n",
            ),
            (
                "app/models/locator.rb",
                "class Locator\n  def locate(url)\n    Fetcher.new.fetch(url, ip: \"192.0.2.1\")\n  end\n\n  \
                 def merged(url)\n    Fetcher.new.merge(url, opts: 1)\n  end\nend\n",
            ),
        ],
        "locator",
    );
    assert!(
        src.contains(r#"Fetcher.new.fetch(url, ip: "192.0.2.1")"#),
        "the keyword must bind the original `ip:` slot:\n{src}"
    );
    assert!(
        src.contains("Fetcher.new.merge(url, opts: 1)"),
        "a genuine optional positional keeps the Hash Ruby hands it:\n{src}"
    );
}
