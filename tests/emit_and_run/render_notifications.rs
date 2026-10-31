//! ActionView's and ActionController's events on the CRuby tree: a page
//! publishes start_processing / process_action, its template and layout,
//! and a `render collection` once as render_collection with its count,
//! never its items as render_partial. `lower::view_instrumentation` and
//! runtime/spinel/active_support_instrumentation.rb; the Spinel half is
//! tests/spinel_active_support_instrumentation.rs.

use super::emit_and_run;

const SCRIPT: &str = r##"
events = []
ActiveSupport::Notifications.monotonic_subscribe(/action_view|action_controller|sql\.active_record/) do |name, start, finish, _id, payload|
  events << [name, (payload[:identifier] || payload[:controller] || payload[:sql]).to_s, start.is_a?(Float) && start <= finish, payload[:count]]
end
Article.create!(title: "Hello", body: "A body that is long enough")
Article.create!(title: "Again", body: "Another body that is long enough")
events.clear
status, _headers, _body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/articles", "QUERY_STRING" => "", "HTTP_HOST" => "localhost", "rack.input" => StringIO.new(""))
raise "status #{status}" unless status == 200
names = events.map { |e| [e[0], e[1]] }
p names
raise "start_processing #{names}" unless names.include?(["start_processing.action_controller", "ArticlesController"])
raise "process_action #{names}" unless names.include?(["process_action.action_controller", "ArticlesController"])
raise "template #{names}" unless names.include?(["render_template.action_view", "app/views/articles/index.html.erb"])
raise "layout #{names}" unless names.include?(["render_layout.action_view", "app/views/layouts/application.html.erb"])
collection = events.find { |e| e[0] == "render_collection.action_view" }
raise "collection #{names}" unless collection && collection[1] == "app/views/articles/_article.html.erb" && collection[3] == 2
raise "items reported one by one #{names}" if names.any? { |n, id| n == "render_partial.action_view" && id.end_with?("_article.html.erb") }
raise "sql #{names}" unless names.any? { |n, _| n == "sql.active_record" }
raise "clock #{events}" unless events.all? { |e| e[2] }
puts "ok"
"##;

#[test]
fn a_page_publishes_its_controller_and_render_events() {
    emit_and_run::real_blog().run_ruby(SCRIPT).assert_passes();
}
