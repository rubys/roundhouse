//! A call site that fills an `id` segment with a String that can be
//! nil (`String | nil`) retypes the segment `String`, as a plain String
//! does (`route_helper_string_call_site_typing`).
//!
//! The survey read only an argument typed exactly `String`, so a
//! nullable string column read off a record that can be nil
//! (`Note.first.route_param`) left the segment on its name-based
//! `Integer` default. The call then passes a String to an
//! `(Integer id)` signature, and Spinel's `--rbs .` rejects it
//! ("parameter id of strike_path is declared Integer but this call
//! passes String"). Rails writes the value with `to_param`:
//! `strike_path("abc")` is `/strikes/abc`.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_routes_to_library_functions;
use roundhouse::ty::Ty;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :notes do |t|\n    t.string :route_param\n  end\n  \
    create_table :strikes do |t|\n    t.string :text\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  \
    resources :strikes, only: :show\nend\n";

// A helper passing a `String | nil` to an `id` segment.
const HELPER: &str = r#"
module NoteHelper
  def note_strike
    note = Note.first
    strike_path(note.route_param) if note.route_param.present?
  end
end
"#;

fn helper_params(name: &str) -> Vec<roundhouse::ty::Param> {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("app/models/strike.rb", "class Strike < ApplicationRecord\nend\n"),
        ("app/helpers/note_helper.rb", HELPER),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let sig = lower_routes_to_library_functions(&app)
        .into_iter()
        .find(|f| f.name.as_str() == name)
        .unwrap_or_else(|| panic!("helper {name} not generated"))
        .signature
        .expect("helper signature");
    let Ty::Fn { params, .. } = sig else { panic!("helper signature is not Ty::Fn") };
    params.to_vec()
}

#[test]
fn a_nilable_string_column_read_retypes_an_id_segment() {
    let params = helper_params("strike_path");
    assert_eq!(params[0].name.as_str(), "id");
    assert!(matches!(*params[0].ty, Ty::Str), "got {:?}", params[0].ty);
}

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The same shape run end to end: one action fills the segment from a
/// nullable string column of a record that can be nil, another from a
/// record. Before, the second compiled against `(Integer id)` and the
/// first contradicted it under Spinel's `--rbs .`.
fn run_example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/note.rb", "class Note < ApplicationRecord\nend\n")
        .write("app/models/strike.rb", "class Strike < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write(
            "app/controllers/notes_controller.rb",
            "class NotesController < ApplicationController
  def show
    note = Note.first
    if note.route_param.present?
      redirect_to strike_path(note.route_param)
    else
      head :not_found
    end
  end
end
",
        )
        .write(
            "app/controllers/strikes_controller.rb",
            "class StrikesController < ApplicationController
  def show
    @strike = Strike.find(params[:id])
    redirect_to strike_path(@strike)
  end
end
",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :strikes, only: :show\n  get \"notes/current\", to: \"notes#show\"\nend\n",
        )
}

const RUN: &str = r##"
require_relative "app/controllers/notes_controller"
require_relative "app/controllers/strikes_controller"

Note.create!(route_param: "abc")
strike = Strike.create!(text: "s")

c = NotesController.new
c.params = {}
c.process_action(:show)
raise "note: " + c.location.inspect unless c.location == "/strikes/abc"

c = StrikesController.new
c.params = {"id" => strike.id.to_s}
c.process_action(:show)
raise "strike: " + c.location.inspect unless c.location == "/strikes/" + strike.id.to_s
puts "strike paths passed"
"##;

#[test]
fn a_nilable_string_segment_runs_in_ruby() {
    run_example().run_ruby(RUN).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_nilable_string_segment_runs_on_spinel_with_rbs() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{RUN}"
    );
    run_example().run_spinel_with_rbs(&script).assert_passes();
}

/// The same evidence through a `direct` helper's `route_for`: the block
/// parameter is seeded from the helper's call site, `Note.first`, so
/// `note.route_param` there is `String | nil` too.
#[test]
fn a_nilable_string_in_a_direct_helpers_route_for_retypes_the_segment() {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  \
               resources :strikes, only: :show\n  \
               direct :fresh_strike do |note, options|\n    \
                 route_for :strike, note.route_param\n  \
               end\nend\n",
        ),
        ("app/models/note.rb", "class Note < ApplicationRecord\nend\n"),
        ("app/models/strike.rb", "class Strike < ApplicationRecord\nend\n"),
        ("app/helpers/note_helper.rb", "module NoteHelper\n  def note_link\n    fresh_strike_path(Note.first)\n  end\nend\n"),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let sig = lower_routes_to_library_functions(&app)
        .into_iter()
        .find(|f| f.name.as_str() == "strike_path")
        .expect("strike_path not generated")
        .signature
        .expect("helper signature");
    let Ty::Fn { params, .. } = sig else { panic!("not Ty::Fn") };
    assert_eq!(params[0].name.as_str(), "id");
    assert!(matches!(*params[0].ty, Ty::Str), "got {:?}", params[0].ty);
}
