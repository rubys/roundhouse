//! `ActionText::Attachment#attachable` dereferences an sgid through
//! `ActionText::Attachable.locate`, which is GENERATED per app: one
//! `when "<Model>"` per model that mixes `ActionText::Attachable` in,
//! written into the emitted tree's `global_id_locator.rb` between its
//! markers. And `obj.extend Mod` on an instance — what campfire's test
//! does to make a `Room` attachable for one assertion — is a raise stub
//! naming the construct, so the file it sits in still links on spinel.

use roundhouse::ingest::ingest_app_from_tree;

fn app_with(files: Vec<(&str, &str)>) -> roundhouse::App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :name\n  end\n  create_table :rooms do |t|\n    t.string :name\n  end\nend\n";

#[test]
fn the_locator_is_a_case_over_the_attachable_models() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        // campfire's layout: the marker one level down, in a module
        // nested under the model.
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include Mentionable\nend\n",
        ),
        (
            "app/models/user/mentionable.rb",
            "module User::Mentionable\n  include ActionText::Attachable\n\n  def to_attachable_partial_path\n    \"users/mention\"\n  end\nend\n",
        ),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
    ]);
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, std::path::Path::new("fixtures/real-blog")).expect("spinel tree");
    let locator = files
        .iter()
        .find(|(p, _)| p.ends_with("global_id_locator.rb"))
        .map(|(_, c)| c.clone())
        .expect("global_id_locator.rb in the tree");
    // The include is resolved through the concern; Room, which does
    // not mix the module in, gets no arm.
    assert!(
        locator.contains(
            "    def self.locate(model_name, id)\n      case model_name\n      when \"User\"\n        User.find_by({ id: id })\n      end\n    end\n"
        ),
        "generated locate:\n{locator}"
    );
    assert!(!locator.contains("when \"Room\""), "Room is not attachable:\n{locator}");
}

#[test]
fn an_instance_extend_stubs_the_test_that_reaches_for_it() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("test/fixtures/rooms.yml", "pets:\n  name: Pets\n"),
        (
            "test/models/room_test.rb",
            "require \"test_helper\"\nclass RoomTest < ActiveSupport::TestCase\n  test \"dynamic\" do\n    room = rooms(:pets).tap { |r| r.extend ActionText::Attachable }\n    assert_equal \"Pets\", room.name\n  end\n\n  test \"static\" do\n    assert_equal \"Pets\", rooms(:pets).name\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let report = diags
        .iter()
        .find(|d| d.message.contains("Object#extend"))
        .expect("the extend is reported");
    assert_eq!(
        report.severity,
        roundhouse::diagnostic::Severity::Warning,
        "a warning, so the strict archive build still writes the tree"
    );
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    // The WHOLE test is the stub — what followed the extend was
    // written against the object it produced — and the test beside
    // it is untouched.
    assert!(
        test.contains("  def test_dynamic\n    raise \"roundhouse: Object#extend not supported (all targets)\"\n  end\n"),
        "the dynamic test should be the raise:\n{test}"
    );
    assert!(!test.contains(".extend"), "no extend survives into the emit:\n{test}");
    assert!(
        test.contains("  def test_static\n    raise \"assert_equal failed\" if \"Pets\" != RoomsFixtures.pets.name\n  end\n"),
        "the static test keeps its body:\n{test}"
    );
}
