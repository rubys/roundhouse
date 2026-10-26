use super::emit_and_run;

/// Enums whose mapping comes from another model: its enum's class
/// method, `.keys` and `.merge` of one, and its Hash constant. These were
/// reported gaps that dropped the whole declaration. Answers are Rails
/// 8.1.4's for the same models and records, including the reader's
/// first label for a value two labels share after a merge.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do
  create_table \"gadgets\", force: :cascade do |t|
    t.integer \"kind\"
    t.integer \"status\"
  end
  create_table \"sprockets\", force: :cascade do |t|
    t.integer \"vendor\"
  end
  create_table \"widgets\", force: :cascade do |t|
    t.integer \"status\"
    t.integer \"category\"
    t.integer \"mode\"
    t.integer \"vendor\"
  end
end
",
        )
        .write(
            "app/models/gadget.rb",
            "class Gadget < ApplicationRecord\n  KINDS = %i[small medium large].freeze\n  enum :kind, KINDS\n  enum :status, %i[draft live archived].freeze\nend\n",
        )
        .write(
            "app/models/sprocket.rb",
            "class Sprocket < ApplicationRecord\n  VENDORS = { acme: 0, globex: 1, initech: 2, unknown: nil }.freeze\n  enum :vendor, VENDORS\nend\n",
        )
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord
  enum :status, Gadget.statuses, prefix: true
  enum :category, Gadget.kinds.merge(Sprocket.vendors), prefix: true
  enum :mode, Sprocket.vendors.keys.freeze, prefix: true
  enum :vendor, Sprocket::VENDORS, prefix: true
end
",
        )
}

#[test]
fn another_models_enum_mapping_behaves_like_rails() {
    app()
        .run_ruby(
            r##"got = []
got << Widget.statuses.to_a << Widget.categories.to_a << Widget.modes.to_a << Widget.vendors.to_a
w = Widget.new
w.category = :globex
w.mode = "unknown"
w.vendor = :unknown
w.status = "archived"
w.save!
w = Widget.find(w.id)
got << [w.category, w.mode, w.vendor, w.status, w.category_globex?, w.mode_unknown?, w.vendor_unknown?]
got << [Widget.category_globex.count, Widget.category_medium.count, Widget.where(mode: 3).count, Widget.vendor_unknown.count]
want = [
  [["draft", 0], ["live", 1], ["archived", 2]],
  [["small", 0], ["medium", 1], ["large", 2], ["acme", 0], ["globex", 1], ["initech", 2], ["unknown", nil]],
  [["acme", 0], ["globex", 1], ["initech", 2], ["unknown", 3]],
  [["acme", 0], ["globex", 1], ["initech", 2], ["unknown", nil]],
  ["medium", "unknown", "unknown", "archived", true, true, true],
  [1, 1, 1, 1],
]
raise "cross-class enums: #{got.inspect}" unless got == want
puts "cross-class enum mappings passed"
"##,
        )
        .assert_passes();
}

/// The strict targets get the same folded mappings, with no emit error.
#[test]
fn strict_targets_emit_the_folded_mappings_without_errors() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Rust, BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Crystal] {
        let (_, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}
