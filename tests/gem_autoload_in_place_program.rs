//! A literal `autoload` in a gem whose file has requires loads that file's
//! program when the constant is first read.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::Path;

const KAFKAKIT: &str = "require \"kafka-kit/producers\"\nrequire \"kafka-kit/environment\"\n";
const WRITERS: &str = r#"module KafkaKit
  module Writers
    autoload(:MockWriter, "kafka-kit/writers/mock_writer")
    autoload(:LogWriter, "kafka-kit/writers/log_writer")
  end
end
"#;
const ABSTRACT: &str =
    "module KafkaKit\n  module Writers\n    class AbstractWriter\n      def label = \"abstract\"\n    end\n  end\nend\n";
const MOCK: &str = "require \"kafka-kit/writers/abstract_writer\"\n\nmodule KafkaKit\n  module Writers\n    class MockWriter < AbstractWriter\n      def label = \"mock\"\n    end\n  end\nend\n";
const LOG: &str = "require \"kafka-kit/writers/abstract_writer\"\n\nmodule KafkaKit\n  module Writers\n    class LogWriter < AbstractWriter\n      def label = \"log\"\n    end\n  end\nend\n";
const ENVIRONMENT: &str = r#"require "kafka-kit/writers"

module KafkaKit
  module Environment
    def self.default_writer(test: true)
      test ? KafkaKit::Writers::MockWriter.new : KafkaKit::Writers::LogWriter.new
    end
  end
end
"#;
const PRODUCERS: &str = r#"module KafkaKit
  module Producers
    autoload(:ClassicProducer, "kafka-kit/producers/classic_producer")
  end
end
"#;
const BASE_PRODUCER: &str =
    "module KafkaKit\n  module Producers\n    class BaseProducer\n      def kind = \"base\"\n    end\n  end\nend\n";
const CLASSIC: &str = "require_relative \"base_producer\"\n\nmodule KafkaKit\n  module Producers\n    class ClassicProducer < BaseProducer\n      def initialize(writer: KafkaKit::Environment.default_writer)\n        @writer = writer\n      end\n\n      def kind = \"classic via #{@writer.label}\"\n    end\n  end\nend\n";

const SCRIPT: &str = "require \"kafka-kit\" unless defined?(KafkaKit::Environment)\n\
puts KafkaKit::Environment.default_writer.label\n\
puts KafkaKit::Environment.default_writer(test: false).label\n\
puts KafkaKit::Writers::MockWriter.superclass.name\n\
puts KafkaKit::Producers::ClassicProducer.new.kind\n";

const LOCK: &str = "PATH\n  remote: vendor/gems/kafka-kit-1.0.0\n  specs:\n    kafka-kit (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  kafka-kit!\n";

const FILES: [(&str, &str); 9] = [
    ("kafka-kit.rb", KAFKAKIT),
    ("kafka-kit/writers.rb", WRITERS),
    ("kafka-kit/writers/abstract_writer.rb", ABSTRACT),
    ("kafka-kit/writers/mock_writer.rb", MOCK),
    ("kafka-kit/writers/log_writer.rb", LOG),
    ("kafka-kit/environment.rb", ENVIRONMENT),
    ("kafka-kit/producers.rb", PRODUCERS),
    ("kafka-kit/producers/base_producer.rb", BASE_PRODUCER),
    ("kafka-kit/producers/classic_producer.rb", CLASSIC),
];

const EXPECTED: [&str; 4] = ["mock", "log", "KafkaKit::Writers::AbstractWriter", "classic via mock"];

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::to_string).collect()
}

/// The script on CRuby, the gem from its own lib/.
fn native(lib: &Path) -> Vec<String> {
    let script = lib.parent().unwrap().join("control.rb");
    std::fs::write(&script, format!("$LOAD_PATH.unshift({:?})\n{SCRIPT}", lib.display().to_string())).unwrap();
    let out = emit_and_run::ruby().arg(&script).output().expect("CRuby control");
    assert!(out.status.success(), "native control failed\n{}", String::from_utf8_lossy(&out.stderr));
    lines(&out.stdout)
}

#[test]
#[ignore = "fork spec: autoload"]
fn a_literal_gem_autoload_of_a_file_with_requires_loads_its_program() {
    let root = std::env::temp_dir().join(format!("roundhouse-gem-autoload-in-place-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("kafka-kit/lib");
    for (path, text) in FILES {
        let file = lib.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    assert_eq!(native(&lib), EXPECTED, "native control");
    let _ = std::fs::remove_dir_all(&root);

    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"kafka-kit\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n");
    for (path, text) in FILES {
        app = app.write(&format!("vendor/gems/kafka-kit-1.0.0/lib/{path}"), text);
    }
    let run = app.run_ruby(SCRIPT);
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    assert_eq!(lines(run.stdout.as_bytes()), EXPECTED, "emitted boot diverges from CRuby\n{}", run.stderr);
    // Each constant's autoload is the gem's own, and loads the file's
    // program (which requires the superclass first).
    let models = std::fs::read_to_string(run.emitted.join("app/models.rb")).unwrap();
    let program = "models/roundhouse_files/vendor/gems/kafka-kit-1.0.0/lib/kafka-kit/writers/mock_writer";
    assert!(models.contains(&format!("[:MockWriter, {program:?}, ")), "{models}");
    assert!(models.contains("[:ClassicProducer, \"models/roundhouse_files/vendor/gems/kafka-kit-1.0.0/lib/kafka-kit/producers/classic_producer\", "), "{models}");
}
