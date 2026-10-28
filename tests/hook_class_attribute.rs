//! A class attribute or cattr an `included` hook declares is the
//! includer's, as natively.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

const ORACLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");

fn native(program: &str) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rh_hook_class_attribute_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.rb"), format!("require {ORACLE:?}\n{program}")).unwrap();
    let out = Command::new("ruby").arg("main.rb").current_dir(&dir).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout
}

const PRELUDE: &str = "RailsOracle.activate(\"activesupport\")\nrequire \"active_support\"\n\
     require \"active_support/core_ext/class/attribute\"\n";

const POSTS: &str = r##"module Posts
  def self.included(base)
    base.class_attribute :ssl_strict
    base.ssl_strict = true

    base.class_attribute :open_timeout, :read_timeout, default: 60
    base.class_attribute :quiet, instance_writer: false
    base.quiet = :yes
    base.class_attribute :logger
  end

  def strict_label = "strict=#{ssl_strict}"
end
"##;

const GATE: &str = "class Gate\n  include Posts\n\n  self.open_timeout = 30\n\n  def timeouts = [open_timeout, read_timeout]\nend\n";
const SUB_GATE: &str = "class SubGate < Gate\n  self.ssl_strict = false\nend\n";
const NONCE: &str = "class Nonce\n  include Posts\nend\n";

const SCRIPT: &str = r##"p [Gate.ssl_strict, SubGate.ssl_strict, Nonce.ssl_strict, Gate.ssl_strict?, SubGate.ssl_strict?]
p [Gate.open_timeout, Gate.read_timeout, SubGate.open_timeout, Nonce.open_timeout, Nonce.read_timeout, Gate.new.timeouts]
g = Gate.new
p [g.ssl_strict, g.strict_label, g.respond_to?(:quiet=), g.quiet, g.quiet?, Gate.quiet?]
g.ssl_strict = :own
p [g.ssl_strict, Gate.ssl_strict, Gate.new.ssl_strict, SubGate.new.strict_label]
Gate.logger = :log
p [Gate.logger, SubGate.logger, Nonce.logger, Gate.instance_variable_get(:@__class_attr_logger)]
p [Gate.method(:ssl_strict).owner, Gate.instance_method(:ssl_strict).owner, Gate.ancestors.take(2)]
p [Posts.respond_to?(:ssl_strict), Posts.method_defined?(:ssl_strict)]
"##;

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn an_included_hook_s_class_attribute_is_the_includer_s_as_native() {
    let classes = [("lib/posts.rb", POSTS), ("lib/gate.rb", GATE), ("lib/sub_gate.rb", SUB_GATE), ("lib/nonce.rb", NONCE)];
    let program: String = classes.iter().map(|(_, source)| *source).collect();
    let expected = native(&format!("{PRELUDE}{program}{SCRIPT}"));
    assert_eq!(
        expected,
        "[true, false, true, true, false]\n\
         [30, 60, 30, 60, 60, [30, 60]]\n\
         [true, \"strict=true\", false, :yes, true, true]\n\
         [:own, true, true, \"strict=false\"]\n\
         [:log, :log, nil, :log]\n\
         [#<Class:Gate>, Gate, [Gate, Posts]]\n\
         [false, false]\n",
        "native control"
    );
    let run = classes.iter().fold(emit_and_run::empty_app(), |overlay, (path, source)| overlay.write(path, source)).run_ruby(SCRIPT);
    assert!(run.success, "emitted tree failed in {}\n{}\n{}", run.emitted.display(), run.stdout, run.stderr);
    assert_eq!(run.stdout, expected, "{}", run.stderr);
}

/// activemerchant's `PaypalExpressCommon` declares under
/// `if base.respond_to?(:class_attribute)`, true natively, with a Rails 2
/// `class_inheritable_accessor` fallback. A gem compiled as source, whose
/// subclass bodies write the attribute (`InContextPaypalExpressGateway`'s
/// `self.test_redirect_url = ...`).
const REDIRECTS: &str = r##"module Redir
  module Redirects
    def self.included(base)
      if base.respond_to?(:class_attribute)
        base.class_attribute :test_url
        base.class_attribute :live_url
      else
        base.class_inheritable_accessor :test_url
        base.class_inheritable_accessor :live_url
      end
      base.live_url = "live"
    end

    def url(test) = test ? test_url : live_url
  end

  class Express
    include Redirects

    self.test_url = "sandbox"
  end

  class InContext < Express
    self.test_url = "incontext"
  end
end
"##;

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn a_respond_to_guarded_hook_declaration_is_its_branch_as_native() {
    let script = "p [Redir::Express.test_url, Redir::InContext.test_url, Redir::InContext.live_url]\n\
                  p [Redir::Express.new.url(true), Redir::InContext.new.url(true), Redir::InContext.new.url(false)]\n";
    let expected = native(&format!("{PRELUDE}{REDIRECTS}{script}"));
    assert_eq!(expected, "[\"sandbox\", \"incontext\", \"live\"]\n[\"sandbox\", \"incontext\", \"live\"]\n", "native control");
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", "PATH\n  remote: vendor/gems/redir-1.0.0\n  specs:\n    redir (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  redir!\n")
        .write("config/application.rb", "require \"redir\"\n")
        .write("vendor/gems/redir-1.0.0/lib/redir.rb", REDIRECTS)
        .run_ruby(script);
    assert!(run.success, "emitted tree failed in {}\n{}\n{}", run.emitted.display(), run.stdout, run.stderr);
    assert!(!run.stderr.contains("file_require_not_carried"), "{}", run.stderr);
    assert_eq!(run.stdout, expected, "{}", run.stderr);
}

/// activemerchant's `PaypalCommonAPI`: `base.cattr_accessor :pem_file`
/// in the hook. ActiveSupport's `mattr_*` define the accessors on
/// `base` over `@@pem_file`, one class variable for `base`, its
/// subclasses and their instances, seeded on `base` (a default once,
/// shared by every name; nil only when unset).
const CREDS: &str = r##"module Creds
  def self.included(base)
    base.cattr_accessor :pem_file
    base.cattr_accessor :signature
    base.mattr_reader :mode, :fallback, default: [:live]
    base.cattr_writer :secret, instance_writer: false
    base.live = "live"
  end
end
"##;
const PAY: &str = "class Pay\n  class_attribute :live\n  include Creds\n\n  def self.secret_value = @@secret\nend\n";
const SUB_PAY: &str = "class SubPay < Pay\nend\n";
/// A class variable the includer already sees (here its superclass's)
/// keeps its value: an unset name is seeded nil only when undefined.
const PRESET: &str = "class Preset\n  @@signature = :preset\nend\n";
const OTHER_PAY: &str = "class OtherPay < Preset\n  class_attribute :live\n  include Creds\nend\n";

#[test]
#[ignore = "fork spec: class-body loops and hooks"]
fn an_included_hook_s_cattr_is_the_includer_s_class_variable_as_native() {
    let script = "SubPay.pem_file = \"cert\"\n\
                  p [Pay.pem_file, SubPay.pem_file, Pay.new.pem_file, OtherPay.pem_file, OtherPay.signature]\n\
                  Pay.new.signature = :sig\n\
                  p [SubPay.signature, Pay.mode, Pay.fallback.equal?(Pay.mode), SubPay.new.mode]\n\
                  Pay.secret = 1\n\
                  p [Pay.secret_value, Pay.new.respond_to?(:secret=), Pay.respond_to?(:mode=), Pay.class_variables.sort]\n\
                  p [Pay.live, OtherPay.class_variables.sort, Preset.class_variables]\n";
    let prelude = format!("{PRELUDE}require \"active_support/core_ext/module/attribute_accessors\"\n");
    let classes = [("lib/creds.rb", CREDS), ("lib/pay.rb", PAY), ("lib/sub_pay.rb", SUB_PAY), ("lib/preset.rb", PRESET), ("lib/other_pay.rb", OTHER_PAY)];
    let program: String = classes.iter().map(|(_, source)| *source).collect();
    let expected = native(&format!("{prelude}{program}{script}"));
    assert_eq!(
        expected,
        "[\"cert\", \"cert\", \"cert\", nil, :preset]\n\
         [:sig, [:live], true, [:live]]\n\
         [1, false, false, [:@@fallback, :@@mode, :@@pem_file, :@@secret, :@@signature]]\n\
         [\"live\", [:@@fallback, :@@mode, :@@pem_file, :@@secret, :@@signature], [:@@signature]]\n",
        "native control"
    );
    let run = classes.iter().fold(emit_and_run::empty_app(), |overlay, (path, source)| overlay.write(path, source)).run_ruby(script);
    assert!(run.success, "emitted tree failed in {}\n{}\n{}", run.emitted.display(), run.stdout, run.stderr);
    assert_eq!(run.stdout, expected, "{}", run.stderr);
}
