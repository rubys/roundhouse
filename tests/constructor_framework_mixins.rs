//! `Klass.new(…, k: v)` on a class whose mixins the constructor-lookup
//! check cannot see into.
//!
//! A. A framework mixin verified not to replace `new` through a hook
//!    (`MIXINS_WITHOUT_CONSTRUCTOR_HOOK`, Rails 8.1.4) no longer makes the
//!    lookup unmodeled for `new`.
//! B. A mixin that supplies an `initialize` (a source module's, or
//!    `url_helpers`' `ActionDispatch::Routing::UrlFor#initialize`) is
//!    shadowed by the class's own plain `def initialize` without `super`:
//!    Ruby runs the class's (Ruby 4.0.7, oracle `initialize_shadowing.rb`).
//!    Any `super`, a prepend, an `initialize` from a concern's
//!    `included do` or `define_method` keeps the refusal.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const REFUSAL: &str = "unmodeled constructor lookup";

fn helper(body: &str) -> String {
    format!("module Kit\n  module Helper\n{body}\n    def help\n      1\n    end\n  end\nend\n")
}

fn req(init: &str, extra: &str) -> String {
    format!("module Kit\n  class Req\n    include Helper\n{extra}\n{init}\n\n    def describe\n      @x\n    end\n  end\nend\n")
}

const CALLER: &str = "module Kit\n  class Caller\n    def go\n      Req.new(:get, \"u\", allow_local: true).describe\n    end\n  end\nend\n";

const PLAIN: &str = "    def initialize(verb, url, allow_local: false)\n      @x = [verb, url, allow_local]\n    end";

fn emit(helper_body: &str, init: &str, extra: &str) -> (String, Vec<String>) {
    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kit/helper.rb", &helper(helper_body))
        .write("app/lib/kit/req.rb", &req(init, extra))
        .write("app/lib/kit/caller.rb", CALLER)
        .emit(BuildTarget::Ruby);
    let caller = std::fs::read_to_string(emitted.join("app/models/kit/caller.rb")).unwrap_or_default();
    (caller, errors)
}

fn assert_bound(helper_body: &str, init: &str, extra: &str) {
    let (caller, errors) = emit(helper_body, init, extra);
    assert!(!errors.iter().any(|e| e.contains(REFUSAL)), "{errors:#?}");
    assert!(caller.contains("Req.new(:get, \"u\", allow_local: true)"), "{caller}");
    let run = emit_and_run::real_blog()
        .write("app/lib/kit/helper.rb", &helper(helper_body))
        .write("app/lib/kit/req.rb", &req(init, extra))
        .write("app/lib/kit/caller.rb", CALLER)
        .run_ruby("Dir[File.join(__dir__, 'app/models/kit/*.rb')].sort.each { |file| require file }\np Kit::Caller.new.go\n");
    run.assert_passes();
    assert_eq!(run.stdout, "[:get, \"u\", true]\n");
}

fn assert_refused(helper_body: &str, init: &str, extra: &str) {
    let (_caller, errors) = emit(helper_body, init, extra);
    assert!(errors.iter().any(|e| e.contains(REFUSAL)), "not refused: {errors:#?}");
}

const ASSET_TAG: &str = "    include ActionView::Helpers::AssetTagHelper";
const URL_HELPERS: &str = "    extend ActiveSupport::Concern\n    included do\n      include Rails.application.routes.url_helpers\n    end";

/// A: a catalog helper included directly.
#[test]
fn a_framework_helper_without_a_constructor_hook_binds() {
    assert_bound(ASSET_TAG, PLAIN, "");
}

/// B: `url_helpers` supplies `initialize`; the class's own, without
/// `super`, shadows it. A large real app's request class has this shape.
#[test]
fn an_own_initialize_without_super_shadows_url_helpers() {
    assert_bound(&format!("{ASSET_TAG}\n{URL_HELPERS}"), PLAIN, "");
}

/// Any `super` passes the call to the mixin's `initialize`.
#[test]
fn any_super_keeps_the_refusal() {
    for init in [
        "    def initialize(verb, url, allow_local: false)\n      super\n      @x = [verb, url, allow_local]\n    end",
        "    def initialize(verb, url, allow_local: false)\n      super()\n      @x = [verb, url, allow_local]\n    end",
        "    def initialize(verb, url, allow_local: false)\n      [1].each { super() }\n      @x = [verb, url, allow_local]\n    end",
        "    def initialize(verb, url, allow_local: false)\n      begin\n        @x = [verb, url, allow_local]\n      rescue StandardError\n        super()\n      end\n    end",
    ] {
        assert_refused(URL_HELPERS, init, "");
    }
}

/// A prepended module comes before the class's own `initialize`.
#[test]
fn a_prepend_keeps_the_refusal() {
    assert_refused(URL_HELPERS, PLAIN, "    prepend Comparable");
}

/// B never covers `new` itself: a mixin outside the catalog may replace
/// it through a hook, whatever the class's own `initialize` does. (An
/// `ActionView::Helpers::*` include is a view-helper marker ingest drops,
/// so a gem's module stands in.)
#[test]
fn an_uncatalogued_mixin_keeps_the_refusal() {
    assert_refused("    include SomeGem::Mixin", PLAIN, "");
}

/// `initialize` from `define_method` is not a plain `def`.
#[test]
fn a_defined_initialize_keeps_the_refusal() {
    assert_refused(
        URL_HELPERS,
        "    define_method(:initialize) do |verb, url, allow_local: false|\n      @x = [verb, url, allow_local]\n    end",
        "",
    );
}

/// An `initialize` a concern's `included do` defines is not the class's own.
#[test]
fn an_initialize_from_a_concerns_included_do_keeps_the_refusal() {
    let (emitted_caller, errors) = emit_and_run::real_blog()
        .write("app/lib/kit/helper.rb", &helper(URL_HELPERS))
        .write(
            "app/lib/kit/ctor.rb",
            "module Kit\n  module Ctor\n    extend ActiveSupport::Concern\n    included do\n      def initialize(verb, url, allow_local: false)\n        @x = [verb, url, allow_local]\n      end\n    end\n  end\nend\n",
        )
        .write(
            "app/lib/kit/req.rb",
            "module Kit\n  class Req\n    include Helper\n    include Ctor\n\n    def describe\n      @x\n    end\n  end\nend\n",
        )
        .write("app/lib/kit/caller.rb", CALLER)
        .emit(BuildTarget::Ruby);
    let _ = emitted_caller;
    assert!(errors.iter().any(|e| e.contains(REFUSAL)), "not refused: {errors:#?}");
}

/// B with a source mixin that supplies `initialize`, run on CRuby and
/// against native Ruby: the class's own runs.
#[test]
fn an_own_initialize_shadows_a_source_mixins_on_ruby() {
    const MIXIN: &str = "module Kit\n  module Supplier\n    def initialize(*args, **kw)\n      @x = [:mixin]\n    end\n  end\nend\n";
    const REQ: &str = "module Kit\n  class Req\n    include Supplier\n\n    def initialize(verb, url, allow_local: false)\n      @x = [verb, url, allow_local]\n    end\n\n    def describe\n      @x\n    end\n  end\nend\n";
    let dir = std::env::temp_dir().join(format!("roundhouse-ctor-mixins-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, src) in [("mixin.rb", MIXIN), ("req.rb", REQ), ("caller.rb", CALLER)] {
        std::fs::write(dir.join(name), src).unwrap();
    }
    let native = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!(
            "%w[mixin req caller].each {{ |f| require File.join({:?}, f) }}\np Kit::Caller.new.go\n",
            dir.display().to_string()
        ))
        .output()
        .expect("native Ruby");
    let _ = std::fs::remove_dir_all(&dir);
    let native = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(native, "[:get, \"u\", true]\n", "native control");
    let run = emit_and_run::real_blog()
        .write("app/lib/kit/supplier.rb", MIXIN)
        .write("app/lib/kit/req.rb", REQ)
        .write("app/lib/kit/caller.rb", CALLER)
        .run_ruby("Dir[File.join(__dir__, \"app/models/kit/*.rb\")].sort.each { |f| require f }\np Kit::Caller.new.go\n");
    run.assert_passes();
    assert_eq!(run.stdout, native, "emitted vs native");
}

/// A module an initializer prepends (`Kit::Req.prepend(Kit::Loud)`,
/// `App::module_mixins`) comes before the class's own `initialize` too.
#[test]
fn an_initializer_prepend_keeps_the_refusal() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/kit/helper.rb", &helper(URL_HELPERS))
        .write("app/lib/kit/req.rb", &req(PLAIN, ""))
        .write("app/lib/kit/loud.rb", "module Kit\n  module Loud\n    def initialize(*args, **kw)\n      @x = [:loud]\n    end\n  end\nend\n")
        .write("app/lib/kit/caller.rb", CALLER)
        .write("config/initializers/kit_loud.rb", "Kit::Req.prepend(Kit::Loud)\n")
        .emit(BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains(REFUSAL)), "not refused: {errors:#?}");
}
