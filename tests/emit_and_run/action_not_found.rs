//! A route that names an action the controller does not define is
//! Rails' `AbstractController::ActionNotFound`: `process` raises it
//! before the callback chain and outside `rescue_from`, and
//! `ActionDispatch::ExceptionWrapper` answers 404. A routed action
//! backed only by a template is still an action and is served.

use super::emit_and_run;

/// `resources :gadgets` routes seven actions. The controller defines
/// `show`, `new` has only a template, and the other five exist nowhere.
/// The `before_action` raises, so a dispatcher that ran the filters
/// before the check would answer 500 instead of 404.
fn gadgets() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  resources :gadgets\n")
        .write(
            "app/controllers/gadgets_controller.rb",
            r#"class GadgetsController < ApplicationController
  before_action :refuse, except: [:show, :new]

  def show
    render json: { ok: true }
  end

  private

  def refuse
    raise ArgumentError, "filter ran"
  end
end
"#,
        )
        .write("app/views/gadgets/new.html.erb", "<p>new gadget</p>\n")
}

#[test]
fn a_routed_action_the_controller_lacks_answers_404() {
    gadgets()
        .run_ruby(
            r##"def call(verb, path)
  status, = Main.run_rack("REQUEST_METHOD" => verb, "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  status
end
{
  ["GET", "/gadgets"] => 404,
  ["GET", "/gadgets/1/edit"] => 404,
  ["POST", "/gadgets"] => 404,
  ["PATCH", "/gadgets/1"] => 404,
  ["DELETE", "/gadgets/1"] => 404,
  ["GET", "/gadgets/1"] => 200,
  ["GET", "/gadgets/new"] => 200,
}.each do |(verb, path), want|
  got = call(verb, path)
  raise "#{verb} #{path} answered #{got}, want #{want}" unless got == want
end
c = GadgetsController.new
begin
  c.process_action(:edit)
  raise "edit fell through"
rescue AbstractController::ActionNotFound => e
  want = "The action 'edit' could not be found for GadgetsController"
  raise e.message unless e.message == want
end
"##,
        )
        .assert_passes();
}

/// The check is the dispatcher's first statement and lists only the
/// actions nothing defines; a controller whose routed actions all exist
/// gets no check, so its dispatcher is unchanged.
#[test]
fn only_a_controller_with_a_missing_routed_action_gets_the_check() {
    let (emitted, errors) = gadgets().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let gadgets = read("app/controllers/gadgets_controller.rb");
    assert!(
        gadgets.contains(
            "  def process_action(action_name)\n    raise AbstractController::ActionNotFound, \"The action '#{action_name}' could not be found for GadgetsController\" if [:create, :destroy, :edit, :index, :update].include?(action_name)\n"
        ),
        "{gadgets}"
    );
    for p in ["app/controllers/articles_controller.rb", "app/controllers/comments_controller.rb"] {
        let src = read(p);
        assert!(src.contains("def process_action("), "{p}:\n{src}");
        assert!(!src.contains("ActionNotFound"), "{p}:\n{src}");
    }
}

/// Which names count as defined: a method spliced in from an included
/// module, whether a namespaced concern under app/controllers/concerns
/// or a module under lib/, is an action, so it gets no check (it still
/// falls through the dispatcher, as before). A routed name that
/// nothing defines answers 404 in the same controller. A controller
/// whose own body includes a module the ingest cannot read (a gem's)
/// gets no check at all, since the action may live there.
fn includers() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  resources :gizmos, only: [:index, :show]\n  resources :widgets, only: [:index, :show]\n  get \"parts/count\", to: \"parts#count\"\n  get \"parts/gone\", to: \"parts#gone\"\n",
        )
        .write(
            "app/controllers/concerns/gizmos/listing.rb",
            "module Gizmos\n  module Listing\n    extend ActiveSupport::Concern\n\n    def index\n      render json: { listed: true }\n    end\n  end\nend\n",
        )
        .write(
            "app/controllers/gizmos_controller.rb",
            "class GizmosController < ApplicationController\n  include Gizmos::Listing\n\n  def show\n    render json: { ok: true }\n  end\nend\n",
        )
        .write(
            "lib/tooling/counting.rb",
            "module Tooling\n  module Counting\n    def count\n      render json: { count: 1 }\n    end\n  end\nend\n",
        )
        .write(
            "app/controllers/parts_controller.rb",
            "class PartsController < ApplicationController\n  include Tooling::Counting\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  include SomeGem::Actions\n\n  def show\n    render json: { ok: true }\n  end\nend\n",
        )
}

#[test]
fn an_action_from_an_included_module_is_defined() {
    includers()
        .run_ruby(
            r##"{
  "/gizmos" => 200,
  "/gizmos/1" => 200,
  "/parts/count" => 200,
  "/parts/gone" => 404,
  "/widgets" => 200,
  "/widgets/1" => 200,
}.each do |path, want|
  got, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  raise "GET #{path} answered #{got}, want #{want}" unless got == want
end
"##,
        )
        .assert_passes();
}

#[test]
fn only_a_routed_name_nothing_defines_gets_the_check() {
    let (emitted, _) = includers().emit(roundhouse::project::BuildTarget::Ruby);
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let parts = read("app/controllers/parts_controller.rb");
    assert!(
        parts.contains("could not be found for PartsController\" if [:gone].include?(action_name)\n"),
        "{parts}"
    );
    for p in ["app/controllers/gizmos_controller.rb", "app/controllers/widgets_controller.rb"] {
        let src = read(p);
        assert!(src.contains("def process_action("), "{p}:\n{src}");
        assert!(!src.contains("ActionNotFound"), "{p}:\n{src}");
    }
}

/// `alias_method :index, :show` makes `index` an action, because `show`
/// is a public action. The alias counts as defined, so only `edit`,
/// which nothing defines, gets the check.
fn aliased() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  resources :gadgets, only: [:index, :show, :edit]\n",
        )
        .write(
            "app/controllers/gadgets_controller.rb",
            "class GadgetsController < ApplicationController\n  def show\n    render json: { ok: true }\n  end\n  alias_method :index, :show\nend\n",
        )
}

#[test]
fn an_alias_of_a_public_action_is_defined() {
    let (emitted, _) = aliased().emit(roundhouse::project::BuildTarget::Ruby);
    let src = std::fs::read_to_string(emitted.join("app/controllers/gadgets_controller.rb")).expect("read");
    assert!(src.contains("could not be found for GadgetsController\" if [:edit].include?(action_name)\n"), "{src}");
    aliased()
        .run_ruby(
            r##"{ "/gadgets" => 200, "/gadgets/1" => 200, "/gadgets/1/edit" => 404 }.each do |path, want|
  got, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  raise "GET #{path} answered #{got}, want #{want}" unless got == want
end
"##,
        )
        .assert_passes();
}
