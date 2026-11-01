//! #1588: `ActiveSupport::TimeZone#tzinfo` and `Time.zone.tzinfo`. Rails'
//! is a TZInfo::Timezone; the runtime answers its `name` and `identifier`,
//! the IANA identifier ("Asia/Tokyo" for "Tokyo", "America/New_York" for
//! "Eastern Time (US & Canada)"), as activesupport 8.1.4 does. A time-zone
//! picker stores that identifier.

use super::emit_and_run;

/// The issue's picker, on real-blog's article form (its `title` stands in
/// for a `time_zone` column), and the configured zone's identifier.
fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/views/articles/_form.html.erb",
        "    <%= form.label :body %>\n",
        concat!(
            "    <%= form.select :title, ActiveSupport::TimeZone.all.collect { |tz| [\"(UTC#{tz.formatted_offset}) #{tz.name}\", tz.tzinfo.name] } %>\n",
            "    <p id=\"zone\"><%= Time.zone.tzinfo.name %></p>\n",
            "    <p id=\"zone-id\"><%= Time.zone.tzinfo.identifier %></p>\n",
            "    <%= form.label :body %>\n",
        ),
    )
}

const TEST: &str = r#"require "test_helper"

class TimeZonePickerControllerTest < ActionDispatch::IntegrationTest
  test "each zone's option carries its IANA identifier" do
    get new_article_url
    assert_response :success
    assert_select "option[value='Atlantic/South_Georgia']", "(UTC-02:00) Mid-Atlantic"
    assert_select "option[value='Asia/Kathmandu']", "(UTC+05:45) Kathmandu"
    assert_select "option[value='America/New_York']", "(UTC-05:00) Eastern Time (US & Canada)"
    assert_select "option[value='Etc/GMT+12']", "(UTC-12:00) International Date Line West"
    assert_select "p#zone", "ZONE"
    assert_select "p#zone-id", "ZONE"
  end
end
"#;

#[test]
fn a_time_zone_picker_stores_the_iana_identifier() {
    app()
        .write("test/controllers/time_zone_picker_controller_test.rb", &TEST.replace("ZONE", "Etc/UTC"))
        .run_test("test/controllers/time_zone_picker_controller_test.rb")
        .assert_passes();
}

/// `Time.zone` is the configured zone: its identifier, not Rails' name.
#[test]
fn the_configured_zone_answers_its_iana_identifier() {
    app()
        .edit(
            "config/application.rb",
            "    # config.time_zone = \"Central Time (US & Canada)\"\n",
            "    config.time_zone = \"Eastern Time (US & Canada)\"\n",
        )
        .write("test/controllers/time_zone_picker_controller_test.rb", &TEST.replace("ZONE", "America/New_York"))
        .run_test("test/controllers/time_zone_picker_controller_test.rb")
        .assert_passes();
}

/// The runtime models only `name`/`identifier`; anything else Rails'
/// TZInfo::Timezone answers (`to_s` is "Asia - Tokyo" there) is refused.
#[test]
fn a_tzinfo_method_the_runtime_does_not_model_is_refused() {
    for call in ["tz.tzinfo.to_s", "tz.tzinfo.utc_offset", "Time.zone.tzinfo.current_period"] {
        let errors = emit_and_run::real_blog()
            .edit(
                "app/views/articles/_form.html.erb",
                "    <%= form.label :body %>\n",
                &format!("    <% tz = ActiveSupport::TimeZone.all.first %><%= {call} %>\n    <%= form.label :body %>\n"),
            )
            .check_errors();
        assert!(errors.iter().any(|e| e.contains("TZInfo::Timezone method")), "{call}: {errors:?}");
    }
}

/// The picker and `Time.zone.tzinfo` in the spinel binary, where #1588
/// was found (`no known method tzinfo on ActiveSupport::TimeZone`).
#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_time_zone_picker_stores_the_iana_identifier_on_spinel() {
    app()
        .run_spinel(concat!(
            "Db.configure(\":memory:\")\n",
            "Schema.statements.each { |sql| Db.exec(sql) }\n",
            "ActiveRecord.adapter = SqliteAdapter\n",
            "controller = ArticlesController.new\n",
            "controller.request_method = \"GET\"\n",
            "controller.process_action(:new)\n",
            "body = controller.body\n",
            "raise body unless body.include?('<option value=\"Asia/Kathmandu\">(UTC+05:45) Kathmandu</option>')\n",
            "raise body unless body.include?('<option value=\"America/New_York\">(UTC-05:00) Eastern Time (US &amp; Canada)</option>')\n",
            "raise body unless body.include?('<p id=\"zone\">Etc/UTC</p>')\n",
            "puts \"tzinfo passed\"\n",
        ))
        .assert_passes();
}

/// `ActiveSupport::TimeZone` is a ruby-family runtime class: Crystal's
/// build of the picker stops at `undefined constant
/// ::ActiveSupport::TimeZone` and Go's at `undefined:
/// ActiveSupportTimeZone_all`. Off the ruby family a `tzinfo` read is
/// refused, naming the target; on Roda too, whose views keep the line as
/// written in an app that loads no ActiveSupport.
#[test]
fn a_tzinfo_read_is_refused_off_the_ruby_family() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Typescript, BuildTarget::Crystal, BuildTarget::Go, BuildTarget::Roda] {
        let (_, errors) = app().emit(target);
        assert!(errors.iter().any(|e| e.contains("tzinfo not supported")), "{target:?}: {errors:?}");
    }
}
