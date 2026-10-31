//! Real native HTTP coverage, run by the existing Spinel toolchain partition.

use super::{emit_and_run, native_http};
use roundhouse::project::BuildTarget;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn rack_mount_native_http_preserves_cookie_fields_and_closes_bodies() {
    let source = r#"class ClosingBody
  def initialize(path)
    @path = path
  end
  def each
    yield "native-body"
    raise "enumeration failed" if @path == "/failure"
  end
  def close
    File.write("rack-body-closed.txt", @path)
  end
end
class StaticFiles
  def self.call(env)
    [200, {"content-type" => "text/plain", "set-cookie" => ["a=1; Path=/", "b=2; Path=/"], "x-repeat" => ["first", "second"], "x-script" => env["SCRIPT_NAME"].to_s, "x-path" => env["PATH_INFO"].to_s}, ClosingBody.new(env["PATH_INFO"].to_s)]
  end
end
class PlainFiles
  def self.call(env)
    [200, {"content-type" => "text/plain"}, ["plain"]]
  end
end
class CachedFiles
  def self.call(env)
    body = [env["HTTP_ACCEPT_LANGUAGE"].to_s + ":" + env["HTTP_X_PAYLOAD"].to_s]
    case env["PATH_INFO"]
    when "/no-cache-split"
      [200, {"cache-control" => ["public, max-age=60", "no-cache"]}, body]
    when "/no-cache-combined"
      [200, {"cache-control" => "public, max-age=60, no-cache"}, body]
    when "/vary-split"
      [200, {"cache-control" => "public, max-age=60", "vary" => ["Accept-Encoding", "Accept-Language"]}, body]
    when "/vary-combined"
      [200, {"cache-control" => "public, max-age=60", "vary" => "Accept-Encoding, Accept-Language"}, body]
    when "/star-split"
      [200, {"cache-control" => "public, max-age=60", "vary" => ["Accept-Encoding", "*"]}, body]
    else
      [200, {"cache-control" => "public, max-age=60", "vary" => "Accept-Encoding, *"}, body]
    end
  end
end
"#;
    let (tree, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n")
        .write("app/controllers/static_files.rb", source)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  mount StaticFiles, at: '/files'\n  mount PlainFiles, at: '/plain'\n  mount CachedFiles, at: '/cached'\nend\n")
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let server = native_http::Server::start(&tree);
    let marker = tree.join("rack-body-closed.txt");
    for (path, remainder) in [("/files", "/"), ("/files/child", "/child")] {
        let _ = std::fs::remove_file(&marker);
        let (response, cookies) = server.get_with_set_cookies(path);
        assert_eq!(response.status, 200, "{}", server.log());
        assert_eq!(response.body, "native-body");
        assert_eq!(response.headers.get("x-script").map(String::as_str), Some("/files"));
        assert_eq!(response.headers.get("x-path").map(String::as_str), Some(remainder));
        assert_eq!(cookies, ["a=1; Path=/", "b=2; Path=/"]);
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), remainder);
    }
    for path in ["/filesystem", "/files0", "/files_"] {
        let _ = std::fs::remove_file(&marker);
        assert_eq!(server.get(path).status, 404, "{path}: {}", server.log());
        assert!(!marker.exists(), "a rejected prefix called the Rack app");
    }
    let plain = server.get("/plain");
    assert_eq!(plain.status, 200);
    assert_eq!(plain.body, "plain");
    for shape in ["split", "combined"] {
        for policy in ["no-cache", "star"] {
            let path = format!("/cached/{policy}-{shape}");
            for payload in ["old", "new"] {
                let response = server.get_with_headers(&path, &[
                    ("Accept-Encoding", "identity"), ("Accept-Language", "en"), ("X-Payload", payload),
                ]);
                assert_eq!(response.status, 200, "{path}: {}", server.log());
                assert_eq!(response.headers.get("x-cache").map(String::as_str), Some("miss"), "{path}");
                assert_eq!(response.body, format!("en:{payload}"), "{path}");
            }
        }
        let path = format!("/cached/vary-{shape}");
        for (language, payload, cache, body) in [
            ("en", "first", "miss", "en:first"),
            ("fr", "second", "miss", "fr:second"),
            ("en", "third", "hit", "en:first"),
            ("fr", "fourth", "hit", "fr:second"),
        ] {
            let response = server.get_with_headers(&path, &[
                ("Accept-Encoding", "identity"), ("Accept-Language", language), ("X-Payload", payload),
            ]);
            assert_eq!(response.status, 200, "{path}: {}", server.log());
            assert_eq!(response.headers.get("x-cache").map(String::as_str), Some(cache), "{path}: {language}");
            assert_eq!(response.body, body, "{path}: {language}");
        }
    }
    let _ = std::fs::remove_file(&marker);
    // The server may terminate the failed response before a status line. The
    // body still belongs to the consumer and must close during unwinding.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.get("/files/failure")));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "/failure", "{}", server.log());
}
