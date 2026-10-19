use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            r##"  root "articles#index"
  get "/outside", to: redirect("https://other.test/landing")
  get "/moved/:id", to: redirect(status: 302) { |params, req| "#{req.protocol}other.test/items/#{params[:id]}" }
  get "/relative", to: redirect("/articles")
  get "/controller-outside", to: "outbound#show"
"##,
        )
        .write(
            "app/controllers/outbound_controller.rb",
            r#"class OutboundController < ApplicationController
  def show
    redirect_to "https://other.test/landing"
  end
end
"#,
        )
}

#[test]
fn routing_redirects_allow_other_hosts_and_controllers_still_refuse_them() {
    app()
        .run_ruby(
            r#"
[
  ["/outside", 301, "https://other.test/landing"],
  ["/moved/7", 302, "http://other.test/items/7"],
  ["/relative", 301, "/articles"]
].each do |path, expected_status, expected_location|
  status, headers, body = Main.run_rack(
    "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "",
    "HTTP_HOST" => "app.test", "rack.input" => StringIO.new("")
  )
  raise "status: #{status}" unless status == expected_status
  raise "location: #{headers['location']}" unless headers["location"] == expected_location
  raise "redirect body" unless body.join.include?("Redirecting")
end
refused = false
begin
  Main.run_rack(
    "REQUEST_METHOD" => "GET", "PATH_INFO" => "/controller-outside", "QUERY_STRING" => "",
    "HTTP_HOST" => "app.test", "rack.input" => StringIO.new("")
  )
rescue ArgumentError => error
  refused = error.message.include?("Unsafe redirect")
end
raise "controller allowed another host" unless refused
puts "routing redirects allow other hosts; controller redirects refuse them"
"#,
        )
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn routing_redirect_host_policy_runs_in_emitted_spinel() {
    app()
        .run_spinel_with_rbs(
            r#"
controller = RoundhouseRedirectsController.new
controller.outside
raise "status" unless controller.status == 301
raise "location" unless controller.location == "https://other.test/landing"
refused = false
begin
  ordinary = ActionController::Base.new
  ordinary.redirect_to("https://other.test/landing")
rescue ArgumentError
  refused = true
end
raise "ordinary controller allowed another host" unless refused
puts "routing host policy passed natively"
"#,
        )
        .assert_passes();
}
