//! A boolean, an Integer that can be nil, or a Float under a route
//! helper's query option reaches the helper as text, the `String?` the
//! helper's option is typed. A nil stays nil, so the helper leaves the
//! key out. Before, only a value typed exactly `Integer` went through
//! `to_s`; the rest reached the helper as they were and contradicted
//! its signature under Spinel's `--rbs .`.
//!
//! Expected paths are what Rails 8.1.4's generated helper writes for the
//! same calls (`widgets_path(featured: true)` is
//! `/widgets?featured=true`, `widgets_path(page: nil)` is `/widgets`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const CONTROLLER: &str = "class WidgetsController < ApplicationController
  def index
    render json: query_paths(2)
  end

  def query_paths(n)
    [
      widgets_path(featured: true),
      widgets_path(featured: false),
      widgets_path(page: n.to_i.positive? ? n.to_i : nil),
      widgets_path(page: (n.to_i - 2).positive? ? n.to_i : nil),
      widgets_path(ratio: 1.5)
    ]
  end
end
";

const ASSERTIONS: &str = r#"
require_relative "app/controllers/widgets_controller"

paths = WidgetsController.new.query_paths(2)
expected = ["/widgets?featured=true", "/widgets?featured=false", "/widgets?page=2", "/widgets", "/widgets?ratio=1.5"]
raise paths.inspect unless paths == expected
puts "query paths passed"
"#;

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", CONTROLLER)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: :index\nend\n")
}

#[test]
fn a_scalar_query_option_reaches_the_helper_as_text() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/widgets_controller.rb")).expect("read the emitted controller");
    for call in [
        "RouteHelpers.widgets_path(featured: true.to_s)",
        "RouteHelpers.widgets_path(featured: false.to_s)",
        "RouteHelpers.widgets_path(ratio: 1.5.to_s)",
    ] {
        assert!(controller.contains(call), "{call}:\n{controller}");
    }
    // The Integer that can be nil is bound once and keeps its nil.
    for n in 0..2 {
        assert!(
            controller.contains(&format!("if route_segment_{n}.nil?")) && controller.contains(&format!("route_segment_{n}.to_s")),
            "route_segment_{n}:\n{controller}"
        );
    }
}

#[test]
fn a_scalar_query_option_runs_in_ruby() {
    example().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_scalar_query_option_runs_on_spinel_with_rbs() {
    example().run_spinel_with_rbs(ASSERTIONS).assert_passes();
}

/// A Hash under a query key is Rails' nested parameters
/// (`widgets_path(filter: { a: 1 })` is `/widgets?filter%5Ba%5D=1`),
/// which the generated helper can't write. The call is left as it is
/// and the survey lists it.
#[test]
fn a_hash_query_option_is_ledgered() {
    use roundhouse::diagnostic::DiagnosticKind;
    use std::collections::HashMap;
    use std::path::PathBuf;

    let controller = "class WidgetsController < ApplicationController
  def index
    redirect_to widgets_path(filter: { kind: params[:kind] })
  end
end
";
    let mut files: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, source) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", controller),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: :index\nend\n"),
    ] {
        files.insert(PathBuf::from(path), source.as_bytes().to_vec());
    }
    let mut app = roundhouse::ingest::ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let (emitted, diags) = roundhouse::emit::diagnostics::scope(|| roundhouse::emit::ruby::emit_lowered_controllers(&app));
    let ledgered: Vec<_> = diags
        .iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::LowerResidue { pass, construct, .. }
            if pass.as_str() == "route_query_option" && construct.as_str() == "widgets_path(filter:)"))
        .collect();
    assert_eq!(ledgered.len(), 1, "{diags:?}");
    let source = emitted
        .iter()
        .find(|f| f.path.ends_with("widgets_controller.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted controller");
    assert!(source.contains("RouteHelpers.widgets_path(filter: {"), "{source}");
}

/// A `direct` helper receives its options hash as written — its block
/// reads `options[:preview]`, and `"false"` is truthy — so a test body's
/// call to one keeps its option values. A generated helper's query
/// option in the same body still reaches it as text.
#[test]
fn a_direct_helper_keeps_its_options_in_a_test_body() {
    use std::collections::HashMap;
    use std::path::PathBuf;

    let routes = "Rails.application.routes.draw do
  resource :account do
    scope module: \"accounts\" do
      resource :logo, only: :show
    end
  end
  direct :fresh_account_logo do |options|
    route_for :account_logo
  end
end
";
    let test = "require \"test_helper\"

class AccountTest < ActiveSupport::TestCase
  test \"paths\" do
    assert_equal \"/account/logo\", fresh_account_logo_path(preview: false, width: 3)
    assert_equal \"/account/logo?preview=false\", account_logo_path(preview: false)
  end
end
";
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", routes),
        ("test/models/account_test.rb", test),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let emitted = roundhouse::emit::ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with("account_test.rb"))
        .expect("no account_test.rb emitted")
        .content
        .clone();
    assert!(!emitted.contains("false.to_s, width"), "the direct helper's options were stringified:\n{emitted}");
    assert!(!emitted.contains("3.to_s"), "the direct helper's options were stringified:\n{emitted}");
    assert!(emitted.contains("account_logo_path(preview: false.to_s)"), "the generated helper's option:\n{emitted}");
}

/// A helper call nested in another's arguments is projected too. The
/// projection replaces the outer call without visiting the replacement,
/// so the nested call's own options are projected first. Checked on the
/// shared test lowering every target runs; the Ruby emit's own pass
/// visits children first and hid this there. A direct helper keeps its
/// options at any depth, inside a generated helper or around one.
#[test]
fn a_nested_helper_call_in_a_test_body_is_projected_too() {
    use std::collections::HashMap;
    use std::path::PathBuf;

    let routes = "Rails.application.routes.draw do
  resource :account do
    scope module: \"accounts\" do
      resource :logo, only: :show
    end
  end
  direct :fresh_account_logo do |options|
    route_for :account_logo
  end
end
";
    let test = "require \"test_helper\"

class AccountTest < ActiveSupport::TestCase
  test \"paths\" do
    a = account_logo_path(preview: false, next: account_logo_path(preview: true))
    b = account_logo_path(preview: false, next: fresh_account_logo_path(preview: true))
    c = fresh_account_logo_path(preview: false, next: account_logo_path(preview: true))
  end
end
";
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", routes),
        ("test/models/account_test.rb", test),
    ]
    .into_iter()
    .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    let app = roundhouse::ingest::ingest_app_from_tree(files).expect("ingest");
    let lcs = roundhouse::lower::lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        Vec::new(),
        &roundhouse::lower::routes::helper_id_segments(&app),
        &roundhouse::lower::routes::direct_helper_names(&app),
    );
    let body = lcs
        .iter()
        .flat_map(|lc| lc.methods.iter())
        .find(|m| m.name.as_str() == "test_paths")
        .map(roundhouse::emit::ruby::emit_method)
        .expect("test_paths lowered");
    for expected in [
        "RouteHelpers.account_logo_path(preview: false.to_s, next: RouteHelpers.account_logo_path(preview: true.to_s))",
        "RouteHelpers.account_logo_path(preview: false.to_s, next: RouteHelpers.fresh_account_logo_path(preview: true))",
        "RouteHelpers.fresh_account_logo_path(preview: false, next: RouteHelpers.account_logo_path(preview: true.to_s))",
    ] {
        assert!(body.contains(expected), "{expected}:\n{body}");
    }
}

/// An `id` / `*_id` query key is Integer by name, but a record whose
/// model answers a slug `to_param` makes the helper declare it
/// `String?`. Another call's Integer under that key then has to be text
/// too, nil kept nil (actionpack 8.1.4: `items_path(widget_id: widget)`
/// is `/items?widget_id=acme`, `widget_id: 3` is `/items?widget_id=3`,
/// `widget_id: nil` is `/items`). The key name had kept the Integer as
/// it was, which Spinel's `--rbs .` refused against `String?`.
const SLUG_ID_CONTROLLER: &str = "class ItemsController < ApplicationController
  def index
    render plain: id_paths(Widget.new(slug: \"acme\"), 3).join(\" \")
  end

  def id_paths(widget, n)
    [
      items_path(widget_id: widget),
      items_path(widget_id: n > 0 ? n : nil),
      items_path(widget_id: n > 5 ? n : nil)
    ]
  end
end
";

const SLUG_ID_ASSERTIONS: &str = r#"
require_relative "app/controllers/items_controller"

paths = ItemsController.new.id_paths(Widget.new(slug: "acme"), 3)
raise paths.inspect unless paths == ["/items?widget_id=acme", "/items?widget_id=3", "/items"]
puts "slug id paths passed"
"#;

fn slug_id_example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"slug\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  def to_param\n    slug\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/items_controller.rb", SLUG_ID_CONTROLLER)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :items, only: :index\nend\n")
}

#[test]
fn an_integer_under_a_slug_typed_id_key_runs_in_ruby() {
    slug_id_example().run_ruby(SLUG_ID_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_integer_under_a_slug_typed_id_key_runs_on_spinel_with_rbs() {
    slug_id_example()
        .run_spinel_with_rbs(&format!(
            "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{SLUG_ID_ASSERTIONS}"
        ))
        .assert_passes();
}
