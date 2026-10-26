//! pundit from its gem source in the Spinel binary: `policy_scope!`
//! resolves the policy's Scope through the closed `safe_constantize`
//! case and `authorize` refuses with Rails' message, the answers
//! `emit_and_run::pundit_source` gets on CRuby. Needs a Spinel
//! compiler, so `#[ignore]`d; CI's `spinel-framework` job runs it
//! (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pundit_app.rs"]
mod pundit_app;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn policy_scope_and_authorize_run_from_the_gem_source_in_the_spinel_binary() {
    pundit_app::app()
        .run_spinel_with_db(
            r#"published = Article.create!(title: "Published", body: "body number one is long enough")
Article.create!(title: "Hidden draft", body: "body number two is long enough")
got = [
  Pundit::Context.new(user: "admin").policy_scope!(Article).count,
  Pundit::Context.new(user: "bob").policy_scope!(Article).map(&:title),
  Pundit::Context.new(user: "nobody").policy_scope!(Article).count,
  Pundit::Context.new(user: "admin").authorize(published, query: :edit?, policy_class: nil).title,
]
denied = begin
  Pundit::Context.new(user: "bob").authorize(published, query: :edit?, policy_class: nil)
  "allowed"
rescue Pundit::NotAuthorizedError => e
  e.message
end
want = [2, ["Published"], 0, "Published"]
raise "got #{got.inspect}" unless got == want
raise "denied: #{denied}" unless denied == "not allowed to ArticlePolicy#edit? this Article"
puts "ok"
"#,
        )
        .assert_passes();
}
