//! A top-level class that shares its leaf name with a namespaced one
//! (`Request` and `Fasp::Request`). Ruby resolves a bare `Request`
//! lexically (Ruby 4.0.7):
//! - in a top-level class, a helper module, and `class Fasp::Compact`
//!   (whose lexical scope is not `Fasp`), it is the top-level `Request`;
//! - inside `module Fasp`, also reopened in another file or nested a level
//!   deeper, it is `Fasp::Request`.
//!
//! Every `Request.new(…, k: v)` was refused as "this unqualified class
//! name is ambiguous". A reference the analyzer resolved
//! (`RESOLVED_CLASS_REF`) now binds to the class it resolved to. A type
//! that only fell back to the written name (a view's `Request`) keeps
//! the refusal.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const TOP: &str = "class Request\n  def initialize(verb, url, allow_local: false, body: nil)\n    @verb = verb\n    @url = url\n    @allow_local = allow_local\n    @body = body\n  end\n\n  def describe\n    [:top, @verb, @url, @allow_local, @body]\n  end\nend\n";
const FASP: &str = "module Fasp\n  class Request\n    def initialize(provider, extra: nil)\n      @provider = provider\n      @extra = extra\n    end\n\n    def describe\n      [:fasp, @provider, @extra]\n    end\n  end\nend\n";
const CHECKER: &str = "class Checker\n  def go\n    Request.new(:get, \"u\", allow_local: true).describe\n  end\nend\n";
const CLIENT: &str = "module Fasp\n  class Client\n    def go\n      Request.new(:p, extra: 1).describe\n    end\n  end\nend\n";
const REOPENED: &str = "module Fasp\n  class Reopened\n    def go\n      Request.new(:r, extra: 2).describe\n    end\n  end\nend\n";
const COMPACT: &str = "class Fasp::Compact\n  def go\n    Request.new(:post, \"c\", body: \"b\").describe\n  end\nend\n";
const DEEP: &str = "module Fasp\n  module Nested\n    class Deep\n      def go\n        Request.new(:d, extra: 3).describe\n      end\n    end\n  end\nend\n";
const HELPER: &str = "module FetchHelper\n  def fetch_it\n    Request.new(:head, \"h\", allow_local: true).describe\n  end\nend\n";

const SCRIPT: &str = "p Checker.new.go\np Fasp::Client.new.go\np Fasp::Reopened.new.go\np Fasp::Compact.new.go\np Fasp::Nested::Deep.new.go\n";
/// A helper module mixed into a view natively; the emitted helper is a
/// module function.
const NATIVE_HELPER: &str = "p Object.new.extend(FetchHelper).fetch_it\n";
const EMITTED_HELPER: &str = "p FetchHelper.fetch_it\n";

const EXPECTED: &str = "[:top, :get, \"u\", true, nil]\n[:fasp, :p, 1]\n[:fasp, :r, 2]\n[:top, :post, \"c\", false, \"b\"]\n[:fasp, :d, 3]\n[:top, :head, \"h\", true, nil]\n";

fn native() -> String {
    let dir = std::env::temp_dir().join(format!("roundhouse-ctor-leaf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut requires = String::new();
    for (i, source) in [TOP, FASP, CHECKER, CLIENT, REOPENED, COMPACT, DEEP, HELPER].iter().enumerate() {
        let path = dir.join(format!("f{i}.rb"));
        std::fs::write(&path, source).unwrap();
        requires.push_str(&format!("require {:?}\n", path.display().to_string()));
    }
    let out = std::process::Command::new("ruby").arg("-e").arg(format!("{requires}{SCRIPT}{NATIVE_HELPER}")).output().expect("native Ruby");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/lib/request.rb", TOP)
        .write("app/lib/fasp/request.rb", FASP)
        .write("app/lib/checker.rb", CHECKER)
        .write("app/lib/fasp/client.rb", CLIENT)
        .write("app/lib/fasp/reopened.rb", REOPENED)
        .write("app/lib/fasp/compact.rb", COMPACT)
        .write("app/lib/fasp/nested/deep.rb", DEEP)
        .write("app/helpers/fetch_helper.rb", HELPER)
}

#[test]
fn a_resolved_leaf_name_binds_the_class_ruby_reaches() {
    assert_eq!(native(), EXPECTED, "native control");
    let run = app().run_ruby(&format!(
        "Dir[File.join(__dir__, \"app/models/**/*.rb\")].sort.each {{ |f| require f }}\n{SCRIPT}{EMITTED_HELPER}"
    ));
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED, "emitted vs native");
}

/// A view's `Request` is typed by the written name, not by the resolver,
/// so whether it is the top-level class is not established: it stays
/// refused.
#[test]
fn a_leaf_name_the_resolver_did_not_answer_stays_refused() {
    let (_emitted, errors) = app()
        .edit(
            "app/views/articles/index.html.erb",
            "<% content_for :title, \"Articles\" %>",
            "<% content_for :title, \"Articles\" %>\n<p><%= Request.new(:get, \"v\", allow_local: true).describe.inspect %></p>",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("unqualified class name is ambiguous")), "{errors:#?}");
}
