//! One Rack::ETag / Rack::ConditionalGet contract for the interpreted
//! and native lanes (`emit_and_run/conditional_get.rs`,
//! `spinel_toolchain/conditional_get.rs`).
//!
//! A Rails 8.1 app's default middleware gives every 200/201 with a body
//! a weak ETag (the first 32 hex digits of the body's SHA-256) and
//! `Cache-Control: max-age=0, private, must-revalidate`, any other
//! response `Cache-Control: no-cache`, and answers a GET whose
//! If-None-Match is exactly that ETag with a bodyless 304. The values
//! below are what Rails 8.1.4 answered for the same actions.
//!
//! The articles index is the real page; its body carries a fresh CSRF
//! token on every request (as in Rails), so the 304 is exercised on a
//! probe action whose body does not change.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/etag-probe/stable\", to: \"etag_probes#stable\"\n  get \"/etag-probe/away\", to: \"etag_probes#away\"\n",
        )
        .write(
            "app/controllers/etag_probes_controller.rb",
            "class EtagProbesController < ApplicationController\n  def stable\n    render plain: \"stable\"\n  end\n\n  def away\n    redirect_to \"/articles\"\n  end\nend\n",
        )
}

pub const DEFAULT_CACHE_CONTROL: &str = "max-age=0, private, must-revalidate";
/// `W/"#{Digest::SHA256.hexdigest("stable")[0, 32]}"`.
pub const STABLE_ETAG: &str = "W/\"f379ccb92b9116442dc65bdc35648a85\"";

/// The Ruby lane: Rack tuples from `Main.run_rack`.
pub const ASSERTIONS: &str = r#"
def get(path, headers = {})
  Main.run_rack({ "REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "",
                  "HTTP_HOST" => "localhost", "rack.input" => StringIO.new("") }.merge(headers))
end

status, headers, body = get("/articles")
raise "index status #{status}" unless status == 200
raise "index etag #{headers.inspect}" unless headers["etag"].to_s.match?(/\AW\/"[0-9a-f]{32}"\z/)
raise "index cache-control #{headers.inspect}" unless headers["cache-control"] == "max-age=0, private, must-revalidate"

status, headers, body = get("/etag-probe/stable")
raise "stable status #{status}" unless status == 200
raise "stable etag #{headers.inspect}" unless headers["etag"] == %(W/"f379ccb92b9116442dc65bdc35648a85")
raise "stable cache-control #{headers.inspect}" unless headers["cache-control"] == "max-age=0, private, must-revalidate"

status, headers, body = get("/etag-probe/stable", "HTTP_IF_NONE_MATCH" => %(W/"f379ccb92b9116442dc65bdc35648a85"))
raise "304 status #{status}" unless status == 304
raise "304 body #{body.inspect}" unless body == [""]
raise "304 content-type #{headers.inspect}" if headers.key?("content-type")
raise "304 etag #{headers.inspect}" unless headers["etag"] == %(W/"f379ccb92b9116442dc65bdc35648a85")

status, headers, body = get("/etag-probe/stable", "HTTP_IF_NONE_MATCH" => %("f379ccb92b9116442dc65bdc35648a85"))
raise "strong tag status #{status}" unless status == 200

status, headers, body = get("/etag-probe/away")
raise "redirect status #{status}" unless status == 302
raise "redirect etag #{headers.inspect}" if headers.key?("etag")
raise "redirect cache-control #{headers.inspect}" unless headers["cache-control"] == "no-cache"
puts "conditional_get contract passed"
"#;
