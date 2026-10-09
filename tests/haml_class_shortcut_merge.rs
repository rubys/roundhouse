//! HAML shortcut-class + hash `class:` merge (Haml 7.5.1 semantics):
//! `.g{ class: k }` folds the `.g` shortcut and the hash `class:` value
//! together through the runtime `haml_class` helper instead of dropping
//! the shortcut. See `src/haml.rs`'s `element` and
//! `runtime/ruby/action_view/view_helpers.rb`'s `haml_class`.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// A `WidgetsController#index` page exercising every merge shape the
/// compiler supports: a scalar (String) value, a false conditional, a
/// ternary, and an Array literal — plus an attribute-order probe and
/// the two byte-identical (no hash `class:` at all) guard elements.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n")
        .write("app/controllers/widgets_controller.rb", r#"class WidgetsController < ApplicationController
  def index
    @key = "alert"
    @flag = params[:flag] == "1"
    @other = params[:other] == "1"
  end
end
"#)
        .write(
            "app/views/widgets/index.html.haml",
            ".gadget{ class: @key }\n\
             .gadget{ class: ('gadget--rtl' if @flag) }\n\
             .gadget{ class: @flag ? 'on' : 'off' }\n\
             .gadget{ class: [@flag && 'gadget--hot', @other && 'gadget--cold'] }\n\
             %a.gadget{ href: '/w', class: @key, title: 't' }\n\
             .gadget{ title: 't' }\n\
             .gadget\n",
        )
}

fn render(flag: &str, other: &str) -> String {
    r#"
require_relative "app/controllers/widgets_controller"
controller = WidgetsController.new
controller.params = {"#.to_string()
        + &format!(r#""flag" => {flag:?}, "other" => {other:?}"#)
        + r#"}
controller.process_action(:index)
raise "status #{controller.status}" unless controller.status == 200
puts controller.body
"#
}

/// Falsy branch: the dynamic value drops out everywhere, leaving only
/// the static shortcut class (never a bare `class=""`).
#[test]
fn falsy_dynamic_values_leave_only_the_static_classes() {
    let run = app().run_ruby(&render("0", "0"));
    run.assert_passes();
    assert_eq!(
        run.stdout.trim_end(),
        concat!(
            "<div class=\"gadget alert\"></div>",
            "<div class=\"gadget\"></div>",
            "<div class=\"gadget off\"></div>",
            "<div class=\"gadget\"></div>",
            "<a href=\"/w\" class=\"gadget alert\" title=\"t\"></a>",
            "<div class=\"gadget\" title=\"t\"></div>",
            "<div class=\"gadget\"></div>",
        ),
        "got:\n{}",
        run.stdout
    );
}

/// Truthy branch: each dynamic value appends after the static class.
#[test]
fn truthy_dynamic_values_append_after_the_static_class() {
    let run = app().run_ruby(&render("1", "1"));
    run.assert_passes();
    assert_eq!(
        run.stdout.trim_end(),
        concat!(
            "<div class=\"gadget alert\"></div>",
            "<div class=\"gadget gadget--rtl\"></div>",
            "<div class=\"gadget on\"></div>",
            "<div class=\"gadget gadget--hot gadget--cold\"></div>",
            "<a href=\"/w\" class=\"gadget alert\" title=\"t\"></a>",
            "<div class=\"gadget\" title=\"t\"></div>",
            "<div class=\"gadget\"></div>",
        ),
        "got:\n{}",
        run.stdout
    );
}

/// Byte-identical guard: an element with a shortcut class and NO hash
/// `class:` key emits exactly what it does without the merge feature —
/// `.gadget{ title: 't' }` and plain `.gadget`, the last two elements
/// in every render above, are asserted there already; this pins them
/// on their own so a future change to the merge path that regresses
/// the no-`class:`-key case fails here specifically.
#[test]
fn an_element_without_a_hash_class_key_is_unaffected() {
    let run = app().run_ruby(&render("0", "0"));
    run.assert_passes();
    assert!(
        run.stdout.contains("<div class=\"gadget\" title=\"t\"></div><div class=\"gadget\"></div>"),
        "got:\n{}",
        run.stdout
    );
}

/// The same page through the Spinel toolchain, when it's available —
/// `spin build` / the `spinel` binary is a separate toolchain this
/// harness does not install, so this probes for it and reports rather
/// than failing a clean checkout.
#[test]
fn spinel_renders_the_same_page() {
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    if std::process::Command::new(&compiler).arg("--version").output().is_err() {
        eprintln!("skipping: no `{compiler}` on PATH (set SPINEL=<path>) — spinel render not probed");
        return;
    }
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        render("1", "1")
    );
    let run = app().run_spinel(&script);
    run.assert_passes();
    assert!(
        run.stdout.contains("<div class=\"gadget gadget--hot gadget--cold\"></div>"),
        "got:\n{}",
        run.stdout
    );
}
