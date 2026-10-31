//! A `direct` helper's `route_for` passes an Integer to a route helper
//! whose segment is typed `String` as its `to_s`, as a call site in a
//! controller, view or helper module already does.
//!
//! A segment is typed `String` when a call site fills it with a String
//! (`strike_path("abc")`); an Integer passed by another caller then gets
//! `to_s` there. The `route_for` rewrite in a `direct` helper builds its
//! call separately and kept `RouteHelpers.strike_path(123)` against
//! `(String id)`, which Spinel's `--rbs .` rejects. Rails writes either
//! with `to_param`: `strike_path(123)` is `/strikes/123`. An optional
//! segment (`/archive(/:period)`) is declared `String | nil` and is a
//! String slot too (`archive_path(2026)` is `/archive/2026`); an
//! Integer that can be nil keeps its nil there (`archive_path(nil)` is
//! `/archive`). A segment that stays `Integer` keeps its Integer.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ROUTES: &str = "Rails.application.routes.draw do
  resources :strikes, only: :show
  resources :notes, only: :show
  direct :first_strike do |options|
    route_for :strike, 123
  end
  direct :first_note do |options|
    route_for :note, 7
  end
  get \"archive(/:period)\", to: \"archives#show\", as: :archive
  direct :this_year do |options|
    route_for :archive, 2026
  end
  direct :pick_archive do |note, options|
    route_for :archive, note&.id
  end
  direct :pick_strike do |note, options|
    route_for :strike, note&.id
  end
  direct :pick_tagged do |note, options|
    route_for_arg_0 = \"?from=helper\"
    route_for(:archive, note&.id) + route_for_arg_0
  end
end
";

// `strike_path`'s segment is a String (a caller passes one); `note_path`'s
// stays the name-based Integer.
const HELPER: &str = "module LinkHelper
  def named_strike
    strike_path(\"abc\")
  end

  def first_note_archive
    pick_archive_path(Note.first)
  end

  def first_note_strike
    pick_strike_path(Note.first)
  end

  def first_note_tagged
    pick_tagged_path(Note.first)
  end
end
";

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"body\"\n  end\nend\n")
        .write("config/routes.rb", ROUTES)
        .write("app/helpers/link_helper.rb", HELPER)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/note.rb", "class Note < ApplicationRecord\nend\n")
}

const RUN: &str = r#"
raise "first_strike: " + RouteHelpers.first_strike_path.inspect unless RouteHelpers.first_strike_path == "/strikes/123"
raise "first_note: " + RouteHelpers.first_note_path.inspect unless RouteHelpers.first_note_path == "/notes/7"
raise "named: " + RouteHelpers.strike_path("abc").inspect unless RouteHelpers.strike_path("abc") == "/strikes/abc"
raise "this_year: " + RouteHelpers.this_year_path.inspect unless RouteHelpers.this_year_path == "/archive/2026"
raise "pick_archive nil: " + RouteHelpers.pick_archive_path(nil).inspect unless RouteHelpers.pick_archive_path(nil) == "/archive"
raise "pick_tagged nil: " + RouteHelpers.pick_tagged_path(nil).inspect unless RouteHelpers.pick_tagged_path(nil) == "/archive?from=helper"
puts "direct route_for passed"
"#;

#[test]
fn route_for_converts_an_integer_only_for_a_string_slot() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let helpers = std::fs::read_to_string(emitted.join("app/route_helpers.rb")).expect("read the emitted route helpers");
    assert!(helpers.contains("RouteHelpers.strike_path(123.to_s)"), "{helpers}");
    assert!(helpers.contains("RouteHelpers.note_path(7)"), "{helpers}");
    // An optional segment is `String | nil`: still a String slot.
    assert!(helpers.contains("RouteHelpers.archive_path(2026.to_s)"), "{helpers}");
    // An Integer that can be nil, into the optional segment, keeps its nil.
    assert!(helpers.contains(".nil?") && helpers.contains("pick_archive_path"), "{helpers}");
    let archive_for = helpers.split("def self.pick_archive_path").nth(1).expect("pick_archive_path emitted");
    let archive_for = archive_for.split("\n  def ").next().unwrap_or(archive_for);
    assert!(archive_for.contains(".nil?") && archive_for.contains(".to_s"), "{archive_for}");
    // Into a REQUIRED String segment it stays as it was: Rails raises on a
    // nil there (a separate question from this conversion).
    assert!(helpers.contains("RouteHelpers.strike_path(note && note.id)"), "{helpers}");
    // A helper that binds `route_for_arg_0` itself keeps it: the temporary
    // takes a name the helper doesn't use.
    let tagged = helpers.split("def self.pick_tagged_path").nth(1).expect("pick_tagged_path emitted");
    let tagged = tagged.split("\n  def ").next().unwrap_or(tagged);
    assert!(tagged.contains("route_for_arg_1 = note && note.id"), "{tagged}");
}

#[test]
fn route_for_into_a_string_slot_runs_in_ruby() {
    example().run_ruby(RUN).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn route_for_into_a_string_slot_runs_on_spinel_with_rbs() {
    example().run_spinel_with_rbs(RUN).assert_passes();
}
