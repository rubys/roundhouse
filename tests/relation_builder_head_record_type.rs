//! A query that starts with a relation builder other than `where` or
//! `all` (`Widget.includes(:gadget).find(id)`, also `preload`,
//! `eager_load`, `order`) keeps its record type in the lowered
//! controller.
//!
//! The lowered bodies are typed against a registry whose models carry
//! only what the runtime's `ActiveRecord::Base` declares class-side
//! (`where`, `all`, `find`, …), not the catalog's other builders, so
//! `Widget.includes(:gadget)` typed `untyped` and so did the record
//! `find` returned from it. A record read through it then reached a
//! route helper's Integer `id` segment unprojected:
//! `edit_gadget_path(@part)` instead of `@part.id`. On CRuby the
//! redirect went to `/gadgets/#<Gadget:0x…>/edit`; Spinel's `--rbs .`
//! rejected the call ("declared Integer but this call passes Gadget").
//! An ivar named like its model (`@gadget`) hid it, through the name
//! rule; any other name (`@part`, or a namespaced model's
//! `@provider` for `Admin::Provider`) has no such fallback.
//!
//! Rails renders the record with `to_param`, its id:
//! `edit_gadget_path(gadget)` is `/gadgets/1/edit`.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const SCHEMA: &str = "ActiveRecord::Schema.define do
  create_table \"gadgets\", force: :cascade do |t|
    t.string \"name\"
  end
  create_table \"orders\", force: :cascade do |t|
    t.string \"code\"
  end
  create_table \"widgets\", force: :cascade do |t|
    t.string \"name\"
    t.bigint \"gadget_id\"
    t.bigint \"order_id\"
  end
end
";

const CONTROLLER: &str = "class WidgetsController < ApplicationController
  def through_includes
    @widget = Widget.includes(:gadget).find(params[:id])
    @part = @widget.gadget
    redirect_to edit_gadget_path(@part)
  end

  def through_preload
    @widget = Widget.preload(:gadget).find(params[:id])
    @thing = @widget.gadget
    redirect_to edit_gadget_path(@thing)
  end

  def through_eager_load
    @widget = Widget.eager_load(:gadget).find(params[:id])
    @thing = @widget.gadget
    redirect_to edit_gadget_path(@thing)
  end

  # `Widget#order` is also an association reader; called on the class
  # with an argument, `order` is still the builder.
  def through_order
    @widget = Widget.order(:id).find(params[:id])
    @thing = @widget.gadget
    redirect_to edit_gadget_path(@thing)
  end

  # A class-side definition named like a builder is the model's own:
  # `Widget.select(name)` answers the record `find_by` finds. (Only its
  # typing is checked here; the emitted call itself is rewritten to the
  # Relation builtin, a separate gap.)
  def through_class_side_override
    @thing = Widget.select(params[:name]).gadget
    redirect_to edit_gadget_path(@thing)
  end
end
";

const ASSERTIONS: &str = r##"
require_relative "app/controllers/widgets_controller"

gadget = Gadget.create!(name: "g")
widget = Widget.create!(name: "w", gadget_id: gadget.id)
expected = "/gadgets/" + gadget.id.to_s + "/edit"

c = WidgetsController.new
c.params = {"id" => widget.id.to_s}
c.process_action(:through_includes)
raise "includes: " + c.location.inspect unless c.location == expected

c = WidgetsController.new
c.params = {"id" => widget.id.to_s}
c.process_action(:through_preload)
raise "preload: " + c.location.inspect unless c.location == expected

c = WidgetsController.new
c.params = {"id" => widget.id.to_s}
c.process_action(:through_eager_load)
raise "eager_load: " + c.location.inspect unless c.location == expected

c = WidgetsController.new
c.params = {"id" => widget.id.to_s}
c.process_action(:through_order)
raise "order: " + c.location.inspect unless c.location == expected


puts "relation builder heads passed"
"##;

const WIDGET: &str = "class Widget < ApplicationRecord
  belongs_to :gadget
  belongs_to :order, optional: true

  def self.select(name)
    find_by(name: name)
  end
end
";

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\nend\n")
        .write("app/models/order.rb", "class Order < ApplicationRecord\nend\n")
        .write("app/models/widget.rb", WIDGET)
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", CONTROLLER)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :gadgets, only: :edit\n  get \"widgets/:id/includes\", to: \"widgets#through_includes\"\n  get \"widgets/:id/preload\", to: \"widgets#through_preload\"\n  get \"widgets/:id/eager_load\", to: \"widgets#through_eager_load\"\n  get \"widgets/:id/order\", to: \"widgets#through_order\"\n  get \"widgets/by_name\", to: \"widgets#through_class_side_override\"\nend\n",
        )
}

#[test]
fn a_record_read_after_a_builder_head_projects_to_its_id() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/widgets_controller.rb")).expect("read the emitted controller");
    assert!(controller.contains("redirect_to(RouteHelpers.edit_gadget_path(@part.id))"), "{controller}");
    assert_eq!(controller.matches("redirect_to(RouteHelpers.edit_gadget_path(@thing.id))").count(), 4, "{controller}");
}

#[test]
fn a_record_read_after_a_builder_head_runs_in_ruby() {
    example().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_record_read_after_a_builder_head_runs_on_spinel_with_rbs() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}"
    );
    example().run_spinel_with_rbs(&script).assert_passes();
}
