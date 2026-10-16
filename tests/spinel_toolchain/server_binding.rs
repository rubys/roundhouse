//! The server listens where `rails server` would: `-b IP`, else BINDING,
//! else localhost in development and 0.0.0.0 in other environments.
//!
//! It always listened on 0.0.0.0. hindmost runs its gateway on the
//! loopback behind Tailscale (`rails server -b 127.0.0.1`), and an
//! every-interface listener put it on the container network too.

use super::{emit_and_run, native_http};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Start the built blog with `envs` and `args`, wait for its banner,
/// and return the address it says it listens on.
fn listening_address(tree: &Path, envs: &[(&str, &str)], args: &[&str]) -> String {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let log = tree.join("binding.log");
    let out = std::fs::File::create(&log).unwrap();
    let mut cmd = Command::new(tree.join("build/bin/blog"));
    cmd.current_dir(tree)
        .env("PORT", port.to_string())
        .env("BLOG_DB", tree.join("binding.sqlite3"))
        .env("SPINEL_WORKERS", "1")
        .env("SECRET_KEY_BASE", "x".repeat(64))
        .env_remove("RAILS_ENV")
        .env_remove("BINDING")
        .args(args)
        .stdout(out.try_clone().unwrap())
        .stderr(out);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("start build/bin/blog");
    let deadline = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(child.try_wait().unwrap().is_none(), "exited:\n{}", std::fs::read_to_string(&log).unwrap());
        assert!(Instant::now() < deadline, "no listener:\n{}", std::fs::read_to_string(&log).unwrap());
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    let text = std::fs::read_to_string(&log).unwrap();
    let at = text.find("listening on http://").unwrap_or_else(|| panic!("no banner:\n{text}"));
    let rest = &text[at + "listening on http://".len()..];
    rest[..rest.find(':').unwrap()].to_string()
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn the_server_binds_where_rails_server_would() {
    let (tree, errors) = emit_and_run::real_blog().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    native_http::build(&tree);
    assert_eq!(listening_address(&tree, &[], &[]), "localhost", "development default");
    assert_eq!(listening_address(&tree, &[("RAILS_ENV", "production")], &[]), "0.0.0.0", "production default");
    assert_eq!(listening_address(&tree, &[("RAILS_ENV", "production"), ("BINDING", "127.0.0.1")], &[]), "127.0.0.1");
    assert_eq!(listening_address(&tree, &[("BINDING", "0.0.0.0")], &["-b", "127.0.0.1"]), "127.0.0.1", "-b wins over BINDING");
}
