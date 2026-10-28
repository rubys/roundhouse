//! A plain gem's files load by its requires, not by autoloading.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const FILES: [(&str, &str); 9] = [
    // The gem's own `autoload :Lazy` keeps working; ak/extra.rb, which
    // nothing requires, defines nothing.
    ("ak.rb", "module Ak\n  autoload :Lazy, \"ak/lazy\"\nend\n\nrequire \"ak/client\"\n"),
    ("ak/lazy.rb", "module Ak\n  module Lazy\n    def self.word\n      \"lazy\"\n    end\n  end\nend\n"),
    ("ak/extra.rb", "module Ak\n  module Extra\n    def self.word\n      \"extra\"\n    end\n  end\nend\n"),
    (
        "ak/client.rb",
        "require \"ak/client/definition\"\nrequire \"ak/client/schema\"\n\nmodule Ak\n  class Client\n    def self.describe\n      Schema.kinds.join(\",\")\n    end\n  end\nend\n",
    ),
    (
        "ak/client/definition.rb",
        "require \"ak/client/schema/object_type\"\n\nmodule Ak\n  class Client\n    class Definition\n      def self.kind\n        Schema::ObjectType.kind\n      end\n    end\n  end\nend\n",
    ),
    (
        "ak/client/schema.rb",
        "require \"ak/client/schema/enum_type\"\nrequire \"ak/client/schema/object_type\"\n\nmodule Ak\n  class Client\n    module Schema\n      def self.kinds\n        [EnumType.kind, ObjectType.kind]\n      end\n    end\n  end\nend\n",
    ),
    (
        "ak/client/schema/base_type.rb",
        "module Ak\n  class Client\n    module Schema\n      module BaseType\n        def label\n          \"base #{kind}\"\n        end\n      end\n    end\n  end\nend\n",
    ),
    (
        "ak/client/schema/enum_type.rb",
        "require \"ak/client/schema/base_type\"\n\nmodule Ak\n  class Client\n    module Schema\n      module EnumType\n        extend BaseType\n\n        def self.kind\n          \"enum\"\n        end\n      end\n    end\n  end\nend\n",
    ),
    (
        "ak/client/schema/object_type.rb",
        "require \"ak/client/schema/base_type\"\n\nmodule Ak\n  class Client\n    module Schema\n      module ObjectType\n        extend BaseType\n\n        def self.kind\n          \"object\"\n        end\n      end\n    end\n  end\nend\n",
    ),
];

const STRICT: &str = "$VERBOSE = true\n\nmodule Strict\n  def warn(msg, category: nil, **)\n    raise \"strict: #{msg}\" if msg.include?(\"circular require\")\n\n    super\n  end\nend\nWarning.extend(Strict)\n";

const SCRIPT: &str = "require \"ak\" unless defined?(Ak)\nputs Ak::Client.describe\nputs Ak::Client::Schema::EnumType.label\nputs Ak::Client::Definition.kind\nputs defined?(Ak::Extra).inspect\nputs Ak::Lazy.word\n";

const LOCK: &str = "PATH\n  remote: vendor/gems/ak-1.0.0\n  specs:\n    ak (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  ak!\n";

fn run(eager: bool) {
    let root = std::env::temp_dir().join(format!("roundhouse-plain-gem-no-autoload-{}-{eager}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in FILES.iter().map(|(p, t)| (format!("lib/{p}"), *t)).chain([("strict.rb".to_string(), STRICT)]) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let native = emit_and_run::ruby()
        .arg("-W")
        .arg("-I")
        .arg(root.join("lib"))
        .arg("-r")
        .arg(root.join("strict.rb"))
        .args(["-e", SCRIPT])
        .output()
        .expect("CRuby control");
    let _ = std::fs::remove_dir_all(&root);
    assert!(native.status.success(), "native control failed:\n{}", String::from_utf8_lossy(&native.stderr));
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(expected, "enum,object\nbase enum\nobject\nnil\nlazy\n", "native control");

    let mut overlay = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write(
            "config/environments/development.rb",
            &format!("Rails.application.configure do\n  config.eager_load = {eager}\nend\n"),
        )
        .write("config/boot.rb", "require_relative \"../lib/strict\"\n")
        .write("config/application.rb", "require_relative \"boot\"\nrequire \"ak\"\n")
        .write("lib/strict.rb", STRICT);
    for (path, text) in FILES {
        overlay = overlay.write(&format!("vendor/gems/ak-1.0.0/lib/{path}"), text);
    }
    let run = overlay.run_ruby(SCRIPT);
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    assert!(!run.stderr.contains("circular require"), "{}", run.stderr);
    assert_eq!(run.stdout, expected, "{}", run.stderr);
}

#[test]
#[ignore = "fork spec: autoload"]
fn a_plain_gems_namespace_reopen_does_not_load_the_namespace_file() {
    run(false);
}

#[test]
#[ignore = "fork spec: autoload"]
fn eager_loading_a_plain_gem_keeps_its_require_order() {
    run(true);
}
