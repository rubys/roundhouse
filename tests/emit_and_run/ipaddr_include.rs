//! `IPAddr#include?` with an `IPAddr` argument, the form forem's
//! `UnifiedEmbed::Tag.blocked_ip?` / `GooglebotVerifier` and lobsters'
//! `Sponge` write. It used to fall through to the Module protocol refusal
//! ("include? requires a proven class/module object receiver").

use super::emit_and_run;

const GUARD: &str = r#"require "ipaddr"

class AddressGuard
  BLOCKED = [IPAddr.new("10.0.0.0/8"), IPAddr.new("100.64.0.0/10"), IPAddr.new("fc00::/7")].freeze

  def self.blocked?(text)
    ip = IPAddr.new(text)
    BLOCKED.any? { |range| range.include?(ip) }
  end

  def self.inside?(net, text)
    IPAddr.new(net).include?(IPAddr.new(text))
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/address_guard.rb", GUARD)
}

const ASSERTIONS: &str = r##"got = [AddressGuard.blocked?("10.1.2.3"), AddressGuard.blocked?("100.127.0.1"), AddressGuard.blocked?("8.8.8.8"),
       AddressGuard.blocked?("fd00::1"), AddressGuard.inside?("10.0.0.0/8", "::ffff:10.1.2.3"), AddressGuard.inside?("192.168.0.0/16", "192.168.255.255")]
raise got.inspect unless got == [true, true, false, true, false, true]
puts "ipaddr include passed"
"##;

#[test]
fn ipaddr_include_runs() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("ipaddr include passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn ipaddr_include_runs_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("ipaddr include passed"), "{}", run.stdout);
}

/// No strict runtime carries the port, so a strict emit reports the class.
#[test]
fn ipaddr_is_unsupported_on_strict_targets() {
    use roundhouse::project::BuildTarget::*;
    for target in [Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Elixir, Crystal] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.iter().any(|e| e.contains("IPAddr")), "{target:?}: {errors:#?}");
    }
}
