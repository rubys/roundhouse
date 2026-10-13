//! A response carries what Rails' would: the default security headers,
//! an absolute redirect Location, and with config.assume_ssl a Secure
//! session cookie and an https:// redirect.
//!
//! The binary sent none of the default headers (X-Frame-Options among
//! them, which keeps another site from framing hindmost's terminal), a
//! relative Location, and treated a request behind a TLS-terminating
//! proxy as plain http.

use super::{emit_and_run, native_http};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

fn head(port: u16, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: blog.test:{port}\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out[..out.find("\r\n\r\n").unwrap_or(out.len())].to_string()
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn responses_carry_rails_headers_and_absolute_redirects() {
    let (tree, errors) = emit_and_run::real_blog()
        .edit("config/environments/production.rb", "# config.assume_ssl = true", "config.assume_ssl = true")
        .edit("config/routes.rb", "root \"articles#index\"", "root \"articles#index\"\n  get \"go\" => \"articles#go\"")
        .edit(
            "app/controllers/articles_controller.rb",
            "  # GET /articles or /articles.json\n",
            "  def go\n    redirect_to \"/articles\"\n  end\n\n  # GET /articles or /articles.json\n",
        )
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut child = Command::new(tree.join("build/bin/blog"))
        .current_dir(&*tree)
        .env("PORT", port.to_string())
        .env("RAILS_ENV", "production")
        .env("SECRET_KEY_BASE", "x".repeat(64))
        .env("BLOG_DB", tree.join("headers.sqlite3"))
        .env("SPINEL_WORKERS", "1")
        .spawn()
        .expect("start");
    let deadline = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "no listener");
        std::thread::sleep(Duration::from_millis(100));
    }
    let page = head(port, "/articles");
    let redirect = head(port, "/go");
    let _ = child.kill();
    let _ = child.wait();
    for h in [
        "X-Frame-Options: SAMEORIGIN",
        "X-XSS-Protection: 0",
        "X-Content-Type-Options: nosniff",
        "X-Permitted-Cross-Domain-Policies: none",
        "Referrer-Policy: strict-origin-when-cross-origin",
    ] {
        assert!(page.contains(h), "{h} missing:\n{page}");
        assert!(redirect.contains(h), "{h} missing on the redirect:\n{redirect}");
    }
    assert!(
        redirect.contains(&format!("Location: https://blog.test:{port}/articles")),
        "absolute, https under assume_ssl:\n{redirect}"
    );
    assert!(page.contains("; Secure"), "the session cookie is Secure under assume_ssl:\n{page}");
}
