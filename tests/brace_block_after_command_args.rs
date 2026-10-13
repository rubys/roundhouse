//! A call's `{ }` block after paren-less arguments.
//!
//! Ruby binds a brace block to the nearest call, so `f a, b { }` passes
//! the block to `b`, and `f a, k: 1 { }` does not parse. Ingest turns a
//! symbol block-pass (`&:upcase`) into a `{ |x| x.upcase }` block of the
//! call. The emitter wrote it after the paren-less arguments, so a DSL
//! call such as `field :name, :string, &:upcase` did not parse. Such a
//! call now takes its arguments in parentheses, so the block is the
//! call's own, as in the source. Native Ruby on the same source is the
//! oracle.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const BUILDER: &str = r##"module Bracelab
  class Builder
    attr_reader :fields

    def initialize
      @fields = []
    end

    def field(name, type = nil, options = {}, &blk)
      @fields << [name, type, options.fetch(:null, true), options[:description], blk ? blk.call("ab") : :no_block]
      self
    end

    def wrap(text)
      block_given? ? yield(text) : text
    end

    def self.report
      b = new
      b.field :a, :string, null: false, &:upcase
      b.field :b, :string, description: "kept", &:reverse
      b.field :c, :string do |v|
        v * 2
      end
      b.field(:d, :string) { |v| v + "!" }
      b.field :e, b.wrap("w") { |t| t + "?" }
      b.field :f, b.wrap("x"), &:length
      b.fields
    end
  end
end
"##;

const REPORT: &str = "Bracelab::Builder.report.each { |f| p f }\n";

#[test]
fn a_brace_block_after_command_arguments_stays_the_calls_own() {
    let dir = std::env::temp_dir().join(format!("roundhouse-brace-block-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let builder = dir.join("builder.rb");
    std::fs::write(&builder, BUILDER).unwrap();
    let native = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!(
            "require {:?}\n{REPORT}",
            builder.display().to_string()
        ))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(
        expected,
        "[:a, :string, false, nil, \"AB\"]\n\
         [:b, :string, true, \"kept\", \"ba\"]\n\
         [:c, :string, true, nil, \"abab\"]\n\
         [:d, :string, true, nil, \"ab!\"]\n\
         [:e, \"w?\", true, nil, :no_block]\n\
         [:f, \"x\", true, nil, 2]\n",
        "native control"
    );

    let (emitted, errors) = emit_and_run::real_blog()
        .write("app/lib/bracelab/builder.rb", BUILDER)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let source = std::fs::read_to_string(emitted.join("app/models/bracelab/builder.rb"))
        .expect("emitted builder");
    let parse = ruby_prism::parse(source.as_bytes());
    let parse_errors: Vec<String> = parse.errors().map(|e| e.message().to_string()).collect();
    assert!(parse_errors.is_empty(), "{parse_errors:?} in:\n{source}");

    let main = emitted.join("main").display().to_string();
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!(
            "require {main:?}\nMain.configure_default_adapter!\nrequire_relative \"app/models/bracelab/builder\"\n{REPORT}"
        ))
        .current_dir(&emitted)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("spawn ruby");
    assert!(
        output.status.success(),
        "emitted program failed in {}:\n{}\n{source}",
        emitted.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected,
        "emitted vs native:\n{source}"
    );
}
