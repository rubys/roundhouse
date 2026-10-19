//! `config.assume_ssl = true` in an environment file reaches the emitted
//! app as `AppEnvironment` in boot.rb, and a relative `redirect_to`
//! becomes that request's absolute URL on the CRuby lane.
//!
//! hindmost sets the flag in production: TLS ends at Tailscale, which
//! forwards plain http, and Rails' AssumeSSL treats every request as https.

use std::collections::HashMap;
use std::path::PathBuf;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn boot_rb(production_rb: &str) -> String {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("config/environments/production.rb", production_rb),
        (
            "config/environments/development.rb",
            "Rails.application.configure do\n  config.assume_ssl = false\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, roundhouse::fixtures::real_blog())
        .expect("spinel tree");
    files.into_iter().find(|(p, _)| p == "boot.rb").map(|(_, c)| c).expect("boot.rb")
}

#[test]
fn assume_ssl_is_carried_for_the_environment_that_sets_it() {
    let boot = boot_rb("Rails.application.configure do\n  config.assume_ssl = true\nend\n");
    assert!(
        boot.contains(
            "module AppEnvironment\n  def self.assume_ssl?(env)\n    env == \"production\"\n  end\n"
        ),
        "{boot}"
    );
}

#[test]
fn a_commented_assume_ssl_is_not() {
    let boot = boot_rb("Rails.application.configure do\n  # config.assume_ssl = true\nend\n");
    assert!(boot.contains("def self.assume_ssl?(env)\n    false\n"), "{boot}");
}

#[test]
fn an_assume_ssl_with_a_trailing_comment_is_carried() {
    let boot = boot_rb(
        "Rails.application.configure do\n  config.assume_ssl = true # TLS ends at the proxy\nend\n",
    );
    assert!(boot.contains("def self.assume_ssl?(env)\n    env == \"production\"\n"), "{boot}");
}

/// The CRuby dispatcher and `redirect_to` share one host policy: a path
/// becomes the request's absolute URL, `X-Forwarded-Host`'s last value
/// wins, `head location:` stays as written, and an action header wins
/// over the Rails 7 default headers.
#[test]
fn redirects_and_default_headers_run_on_the_ruby_lane() {
    let run = emit_and_run::real_blog()
        .edit(
            "config/environments/production.rb",
            "# config.assume_ssl = true",
            "config.assume_ssl = true",
        )
        .run_ruby(
            r#"
ENV["RAILS_ENV"] = "production"
raise "production assume_ssl" unless AppEnvironment.serving_assume_ssl?
ENV["RAILS_ENV"] = "development"
raise "development does not assume ssl" if AppEnvironment.serving_assume_ssl?

bare = ActionController::Base.new
bare.redirect_to "/somewhere"
raise "no host keeps the path: #{bare.location.inspect}" unless bare.location == "/somewhere"

c = ActionController::Base.new
c.request = ActionDispatch::Request.new({ "HTTP_HOST" => "blog.test", "HTTP_X_FORWARDED_PROTO" => "https" })
c.redirect_to "/articles"
raise "absolute https redirect: #{c.location.inspect}" unless c.location == "https://blog.test/articles"

fwd = ActionController::Base.new
fwd.request = ActionDispatch::Request.new({
  "HTTP_HOST" => "internal:3000",
  "HTTP_X_FORWARDED_HOST" => "first.test, public.test"
})
fwd.redirect_to "/login"
raise "forwarded host: #{fwd.location.inspect}" unless fwd.location == "http://public.test/login"
raise "url uses that host" unless fwd.request.url == "http://public.test/"
fwd.redirect_to fwd.request.url
raise "return path matches the public url" unless fwd.location == "http://public.test/"

spoof = ActionController::Base.new
spoof.request = ActionDispatch::Request.new({
  "HTTP_HOST" => "blog.test",
  "HTTP_X_FORWARDED_HOST" => "evil.com\" onclick=\"x",
  "REMOTE_ADDR" => "203.0.113.5",
  "PATH_INFO" => "/login"
})
spoof.redirect_to "/login"
raise "public peer keeps Host: #{spoof.location.inspect}" unless spoof.location == "http://blog.test/login"
blank = ActionController::Base.new
blank.request = ActionDispatch::Request.new({
  "HTTP_HOST" => "blog.test",
  "HTTP_X_FORWARDED_HOST" => "evil.com",
  "REMOTE_ADDR" => ""
})
blank.redirect_to "/login"
raise "blank peer keeps Host: #{blank.location.inspect}" unless blank.location == "http://blog.test/login"
raise "url keeps Host" unless spoof.request.url == "http://blog.test/login"

headed = ActionController::Base.new
headed.request = c.request
headed.head(:created, location: "/made")
raise "head location stays relative: #{headed.location.inspect}" unless headed.location == "/made"

page = ActionController::Base.new
page.render("hi")
page.headers["X-Frame-Options"] = "DENY"
page.commit_default_headers!
raise "action header wins" unless page.headers["X-Frame-Options"] == "DENY"
raise "default nosniff" unless page.headers["X-Content-Type-Options"] == "nosniff"
raise "default referrer" unless page.headers["Referrer-Policy"] == "strict-origin-when-cross-origin"
puts "response headers passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("response headers passed"), "{}\n{}", run.stdout, run.stderr);
    let boot = std::fs::read_to_string(run.emitted.join("boot.rb")).expect("boot.rb");
    assert!(boot.contains("def self.serving_assume_ssl?"), "{boot}");
    assert!(boot.contains("env == \"production\""), "{boot}");
}
