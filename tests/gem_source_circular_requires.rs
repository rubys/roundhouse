//! A gem's circular and self requires stay quiet no-ops, as natively.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

use roundhouse::project::BuildTarget;

const CIRCKIT: &str = "require \"circkit/index\"\nrequire \"circkit/detector\"\n\nmodule Circkit\n  def self.summary = [Index.name, Detector.checks].join(\" \")\nend\n";
const INDEX: &str = "require \"circkit/index\"\n\nmodule Circkit\n  class Index\n  end\nend\n";
const DETECTOR: &str = "require \"circkit/checksum\"\n\nmodule Circkit\n  class Detector\n    def self.checks = Checksum.name\n  end\nend\n";
const CHECKSUM: &str = "require \"circkit\"\n\nmodule Circkit\n  class Checksum\n  end\nend\n";

const LOCK: &str = "PATH\n  remote: vendor/gems/circkit-1.0.0\n  specs:\n    circkit (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  circkit!\n";

/// Counts circular-require warnings into `$circular` and, with `STRICT`
/// set, raises on one as a StrictWarning without an exemption would.
const HOOK: &str = "$circular = 0\nmodule CircularHook\n  def warn(msg, category: nil, **)\n    if msg.include?(\"circular require\")\n      $circular += 1\n      raise \"strict warning: #{msg}\" if ENV[\"STRICT\"]\n    end\n    super\n  end\nend\nWarning.singleton_class.prepend(CircularHook)\n$VERBOSE = true\n";

const SCRIPT: &str = "puts Circkit.summary\n";

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::to_string).collect()
}

#[test]
#[ignore = "fork spec: load order"]
fn a_gem_sources_circular_and_self_requires_stay_quiet_no_ops() {
    let root = std::env::temp_dir().join(format!("roundhouse-gem-source-circular-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("circkit/lib");
    std::fs::create_dir_all(source.join("circkit")).unwrap();
    for (path, text) in [("circkit.rb", CIRCKIT), ("circkit/index.rb", INDEX), ("circkit/detector.rb", DETECTOR), ("circkit/checksum.rb", CHECKSUM)] {
        std::fs::write(source.join(path), text).unwrap();
    }

    // CRuby on the gem's own files: each of the two circular requires is a
    // no-op that warns.
    let native = emit_and_run::ruby()
        .arg("-I")
        .arg(&source)
        .args(["-e", &format!("{HOOK}require \"circkit\"\n{SCRIPT}puts $circular\n")])
        .output()
        .expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(lines(&native.stdout), ["Circkit::Index Circkit::Checksum", "2"], "native control");

    let (tree, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"circkit\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write("vendor/gems/circkit-1.0.0/lib/circkit.rb", CIRCKIT)
        .write("vendor/gems/circkit-1.0.0/lib/circkit/index.rb", INDEX)
        .write("vendor/gems/circkit-1.0.0/lib/circkit/detector.rb", DETECTOR)
        .write("vendor/gems/circkit-1.0.0/lib/circkit/checksum.rb", CHECKSUM)
        .emit(BuildTarget::Ruby);
    let index = std::fs::read_to_string(tree.join("app/models/roundhouse_files/vendor/gems/circkit-1.0.0/lib/circkit/index.rb"))
        .unwrap_or_else(|e| panic!("index program: {e}\nerrors: {errors:#?}"));
    assert!(index.contains("names this gem file itself"), "the self-require is kept:\n{index}");
    let run = Command::new("ruby")
        .current_dir(&tree)
        .env("STRICT", "1")
        .env("BLOG_DB", ":memory:")
        .arg("-e")
        .arg(format!("{HOOK}require File.expand_path(\"main\", Dir.pwd)\nMain.configure_default_adapter!\nrequire \"circkit\" unless defined?(Circkit)\n{SCRIPT}puts $circular\n"))
        .output()
        .expect("spawn ruby");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "emitted tree failed in {}: {stderr}\nerrors: {errors:#?}", tree.display());
    assert_eq!(lines(&run.stdout), ["Circkit::Index Circkit::Checksum", "0"], "emitted Ruby diverges from CRuby\nerrors: {errors:#?}");
    let _ = std::fs::remove_dir_all(&root);
}
