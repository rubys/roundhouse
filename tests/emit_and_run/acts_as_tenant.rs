use super::emit_and_run;

/// `acts_as_tenant :account` (acts_as_tenant 1.0.1) and
/// `validates_uniqueness_to_tenant`. The macro was a dropped model-DSL
/// call: every read saw every tenant's rows and a create was not stamped.
/// The answers are the gem's: reads filter on the current tenant's key and
/// read every row with none set, `without_tenant` lifts the filter for its
/// block and `with_tenant` sets it, a create takes the current tenant's
/// key over the one it was given (`before_validation ... on: :create`),
/// the tenant is a required `belongs_to`, and uniqueness is per tenant.
pub(crate) fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "Gemfile.lock",
            "GEM\n  remote: https://rubygems.org/\n  specs:\n    acts_as_tenant (1.0.1)\n      activerecord (>= 6.0)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  acts_as_tenant (~> 1.0)\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"widgets\", force: :cascade do |t|\n    t.integer \"account_id\"\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/models/account.rb", "class Account < ApplicationRecord\n  has_many :widgets\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  acts_as_tenant :account\n  validates_uniqueness_to_tenant :name\nend\n")
}

pub(crate) const SCRIPT: &str = r#"a = Account.create!(name: "a")
b = Account.create!(name: "b")
ActsAsTenant.with_tenant(a) { Widget.create!(name: "w1") }
wb = ActsAsTenant.with_tenant(b) { Widget.create!(name: "w1") }
outside = ActsAsTenant.current_tenant.nil?
ActsAsTenant.current_tenant = a
missing = begin
  Widget.find(wb.id)
  "found"
rescue ActiveRecord::RecordNotFound
  "not found"
end
stamped = Widget.create!(name: "w2", account_id: b.id)
dup = Widget.new(name: "w1")
dup_valid = dup.valid?
got = [
  outside,
  Widget.count,
  Widget.order(:name).pluck(:name),
  missing,
  Widget.find_by(id: wb.id).nil?,
  ActsAsTenant.without_tenant { Widget.count },
  ActsAsTenant.current_tenant.name,
  stamped.account_id == a.id,
  dup_valid,
  dup.errors.to_a,
  Widget.includes(:account).order(name: :asc).map(&:name),
  Widget.unscoped.count,
]
want = [true, 2, ["w1", "w2"], "not found", true, 3, "a", true, false, ["Name has already been taken"], ["w1", "w2"], 3]
raise "tenant: #{got.inspect}" unless got == want
ActsAsTenant.current_tenant = nil
orphan = Widget.new(name: "o")
orphan_valid = orphan.valid?
none = [orphan_valid, orphan.errors.to_a, Widget.count]
raise "no tenant: #{none.inspect}" unless none == [false, ["Account must exist"], 3]
puts "acts_as_tenant passed"
"#;

#[test]
fn acts_as_tenant_scopes_stamps_and_validates_like_the_gem() {
    app().run_ruby(SCRIPT).assert_passes();
}
