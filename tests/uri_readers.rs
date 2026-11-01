//! Narrowing a gradual URI must retain its inherited component readers.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use roundhouse::ty::Ty;

const READERS: &[&str] = &[
    "scheme", "host", "port", "path", "query", "fragment", "userinfo", "user", "password", "to_s",
];

fn uri_app(readers: &[&str]) -> emit_and_run::Overlay {
    let mut source = String::from("class UriReaderProbe < ApplicationRecord\n");
    for (name, class) in [("generic", "Generic"), ("http", "HTTP"), ("https", "HTTPS")] {
        let mut calls: Vec<String> = readers.iter().map(|r| format!("uri.{r}")).collect();
        if class != "Generic" && readers.contains(&"to_s") {
            calls.push("uri.request_uri".into());
        }
        source.push_str(&format!(
            "  def self.{name}(url)\n    uri = URI.parse(url)\n    if uri.is_a?(URI::{class})\n      [{}]\n    end\n  end\n",
            calls.join(", ")
        ));
    }
    source.push_str("end\n");
    emit_and_run::real_blog().write("app/models/uri_reader_probe.rb", &source)
}

#[test]
fn uri_readers_survive_http_and_https_guards_and_run() {
    uri_app(READERS)
        .run_ruby(r##"
http = "http://alice:secret@example.test:8080/a?b=1#c"
https = "https://alice:secret@example.test:8443/a?b=1#c"
raise unless UriReaderProbe.http(http) == ["http", "example.test", 8080, "/a", "b=1", "c", "alice:secret", "alice", "secret", http, "/a?b=1"]
raise unless UriReaderProbe.https(https) == ["https", "example.test", 8443, "/a", "b=1", "c", "alice:secret", "alice", "secret", https, "/a?b=1"]
raise unless UriReaderProbe.http(https) == UriReaderProbe.https(https)
raise unless UriReaderProbe.http("http://example.test") == ["http", "example.test", 80, "", nil, nil, nil, nil, nil, "http://example.test", "/"]
raise unless UriReaderProbe.generic("custom:opaque") == ["custom", nil, nil, nil, nil, nil, nil, nil, nil, "custom:opaque"]
raise unless UriReaderProbe.generic("/relative") == [nil, nil, nil, "/relative", nil, nil, nil, nil, nil, "/relative"]
raise unless UriReaderProbe.https(http).nil?
"##)
        .assert_passes();
}

#[test]
fn uri_reader_types_preserve_nil_and_parse_stays_gradual() {
    let (_emitted, app, errors) = uri_app(READERS).emit_with_app(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let probe = app
        .models
        .iter()
        .find(|c| c.name.0.as_str() == "UriReaderProbe")
        .unwrap();
    fn visit(expr: &roundhouse::expr::Expr, seen: &mut usize) {
        if let roundhouse::expr::ExprNode::Send { recv, method, .. } = &*expr.node {
            if method.as_str() == "parse" {
                assert_eq!(expr.ty.as_ref(), Some(&Ty::untyped()));
            }
            if let Some(Ty::Class { id, .. }) = recv.as_ref().and_then(|r| r.ty.as_ref()) {
                if matches!(id.0.as_str(), "URI::Generic" | "URI::HTTP" | "URI::HTTPS")
                    && (READERS.contains(&method.as_str()) || method.as_str() == "request_uri")
                {
                    let expected = if method.as_str() == "to_s" {
                        Ty::Str
                    } else {
                        Ty::Union {
                            variants: vec![
                                if method.as_str() == "port" {
                                    Ty::Int
                                } else {
                                    Ty::Str
                                },
                                Ty::Nil,
                            ]
                            .into(),
                        }
                    };
                    assert_eq!(
                        expr.ty.as_ref(),
                        Some(&expected),
                        "{}#{}",
                        id.0.as_str(),
                        method.as_str()
                    );
                    *seen += 1;
                }
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, seen));
    }
    let mut seen = 0;
    for method in probe.methods() {
        visit(&method.body, &mut seen);
    }
    assert_eq!(seen, 32);
}

#[test]
fn uri_readers_remain_refused_without_a_target_runtime() {
    for target in BuildTarget::ALL.iter().copied().chain([BuildTarget::Roda]) {
        if matches!(
            target,
            BuildTarget::Blog
                | BuildTarget::Ruby
                | BuildTarget::Jruby
                | BuildTarget::Spinel
                | BuildTarget::Roda
        ) {
            continue;
        }
        let (_emitted, errors) = uri_app(READERS).emit(target);
        for reader in READERS.iter().copied().chain(["request_uri"]) {
            let owner = if reader == "request_uri" {
                "URI::HTTP"
            } else {
                "URI::Generic"
            };
            assert!(
                errors.iter().any(|e| e.contains(&format!(
                    "{owner}#{reader} not supported ({})",
                    target.as_str()
                ))),
                "{}: {reader}: {errors:#?}",
                target.as_str()
            );
        }
    }
}

#[test]
fn spinel_inherits_the_uri_readers_its_package_implements() {
    let (_emitted, errors) = uri_app(READERS).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn generic_does_not_gain_the_http_request_uri_reader() {
    let errors = emit_and_run::real_blog()
        .write(
            "app/models/uri_reader_probe.rb",
            r#"class UriReaderProbe < ApplicationRecord
  def self.request(url)
    uri = URI.parse(url)
    if uri.is_a?(URI::Generic)
      uri.request_uri
    end
  end
end
"#,
        )
        .check_errors();
    assert!(
        errors.iter().any(|e| e.contains("request_uri")),
        "{errors:#?}"
    );
}

fn uri_union_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/uri_reader_probe.rb",
            r#"class UriReaderProbe < ApplicationRecord
  def self.read_host(uri)
    uri.host
  end
end
"#,
        )
        .write(
            "sig/uri_reader_probe.rbs",
            r#"class UriReaderProbe
  def self.read_host: (URI::HTTP | URI::HTTPS) -> String?
end
"#,
        )
}

fn unsupported_uri_targets() -> impl Iterator<Item = BuildTarget> {
    BuildTarget::ALL.iter().copied().filter(|target| {
        !matches!(
            target,
            BuildTarget::Blog | BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel
        )
    })
}

#[test]
fn uri_union_parameter_is_refused_without_a_target_runtime() {
    let mut missing = Vec::new();
    for target in unsupported_uri_targets() {
        let (_emitted, errors) = uri_union_app().emit(target);
        let expected = format!("URI::Generic#host not supported ({})", target.as_str());
        if !errors.iter().any(|error| error.contains(&expected)) {
            missing.push((target.as_str(), errors));
        }
    }
    assert!(
        missing.is_empty(),
        "union URI readers were not refused: {missing:#?}"
    );
}

#[test]
fn uri_union_parameter_runs_on_ruby() {
    uri_union_app()
        .run_ruby(
            r#"
raise unless UriReaderProbe.read_host(URI.parse("http://example.test/a")) == "example.test"
raise unless UriReaderProbe.read_host(URI.parse("https://example.test/a")) == "example.test"
raise unless UriReaderProbe.read_host(URI.parse("http:/a")).nil?
"#,
        )
        .assert_passes();
}

#[test]
fn uri_union_exempts_only_the_app_defined_class_arms() {
    for target in unsupported_uri_targets() {
        let owned_http = "class URI::HTTP\n  def host\n    \"owned-http\"\n  end\nend\n";
        let (_emitted, mixed_errors) = uri_union_app()
            .write("app/services/owned_http.rb", owned_http)
            .emit(target);
        let expected = format!("URI::Generic#host not supported ({})", target.as_str());
        assert!(
            mixed_errors.iter().any(|error| error.contains(&expected)),
            "{}: the builtin HTTPS arm still needs a runtime: {mixed_errors:#?}",
            target.as_str()
        );

        let (_emitted, owned_errors) = uri_union_app()
            .write("app/services/owned_http.rb", owned_http)
            .write(
                "app/services/owned_https.rb",
                "class URI::HTTPS\n  def host\n    \"owned-https\"\n  end\nend\n",
            )
            .emit(target);
        assert!(
            !owned_errors.iter().any(|error| error.contains(&expected)),
            "{}: app-defined URI classes must remain exempt: {owned_errors:#?}",
            target.as_str()
        );
    }
}

fn uri_owned_host_app(param: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/uri_reader_probe.rb",
            r#"class UriReaderProbe < ApplicationRecord
  def self.read_host(uri)
    uri.host
  end

  def self.read_port(uri)
    uri.port
  end
end
"#,
        )
        .write(
            "sig/uri_reader_probe.rbs",
            &format!(
                "class UriReaderProbe\n  def self.read_host: ({param}) -> String?\n  def self.read_port: ({param}) -> Integer?\nend\n"
            ),
        )
        .write(
            "app/services/owned_http.rb",
            "class URI::HTTP\n  def host\n    \"owned-http\"\n  end\nend\n",
        )
}

#[test]
fn an_app_defined_uri_class_exempts_only_the_readers_it_defines() {
    // HTTPS defining `port` does not supply it to the HTTP arm.
    let owned_https = "class URI::HTTPS\n  def host\n    \"owned-https\"\n  end\n\n  def port\n    8443\n  end\nend\n";
    let mut wrong = Vec::new();
    for target in unsupported_uri_targets() {
        let single = uri_owned_host_app("URI::HTTP");
        let union = uri_owned_host_app("URI::HTTP | URI::HTTPS")
            .write("app/services/owned_https.rb", owned_https);
        for (shape, app) in [("single", single), ("union", union)] {
            let (_emitted, errors) = app.emit(target);
            let refused = |reader: &str| {
                let expected = format!("URI::Generic#{reader} not supported ({})", target.as_str());
                errors.iter().any(|error| error.contains(&expected))
            };
            if refused("host") || !refused("port") {
                wrong.push((target.as_str(), shape, refused("host"), refused("port")));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "(target, receiver, host refused, port refused); only host is the app's: {wrong:#?}"
    );
}
