//! A regex constant that interpolates string constants, with the `u`
//! flag, against native Ruby.
//!
//! A gem builds
//! `UTF8_REGEX = /[^[#{WORDS_REGEX}‍#{EMOJI_REGEX}]]+/u` out of two
//! string constants and `gsub`s with it. Ingest refused every
//! interpolated regex carrying an encoding flag, so the constant was
//! dropped and each read of it was "constant not supported". `u` only
//! fixes the encoding (`Regexp::FIXEDENCODING`), so it is carried as that
//! option bit; `‍` stays regex source for the engine to read.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const HANDLE: &str = r#"# frozen_string_literal: true

module Handle
  EMOJI_REGEX = '\u{00A9}\u{00AE}\u{2194}-\u{2199}\u{1F300}-\u{1F321}'
  WORDS_REGEX = '\p{Word}'
  UTF8_REGEX = /[^[#{WORDS_REGEX}‍#{EMOJI_REGEX}]]+/u
  FLAGGED = /#{WORDS_REGEX}+ x/imu
  PLAIN = /#{WORDS_REGEX}+/

  def self.handle(input)
    input.gsub(UTF8_REGEX, "-").gsub(/\A-+|-+\z/, "")
  end
end
"#;

const SCRIPT: &str = r#"
p Handle.handle("Hello, World! café © \u{1F300}")
p Handle::UTF8_REGEX.source
p Handle::UTF8_REGEX.options
p Handle::UTF8_REGEX.fixed_encoding?
p Handle::FLAGGED.options
p Handle::PLAIN.options
p Handle::FLAGGED.match?("ab X")
"#;

#[test]
fn an_interpolated_regex_constant_with_the_u_flag_works_as_in_ruby() {
    let dir = std::env::temp_dir().join(format!("rh-interpolated-regex-native-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("handle.rb"), HANDLE).unwrap();
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require {:?}\n{SCRIPT}", dir.join("handle.rb").display().to_string()))
        .output()
        .expect("ruby");
    let _ = std::fs::remove_dir_all(&dir);
    let native_out = String::from_utf8_lossy(&native.stdout).into_owned();
    assert!(native.status.success(), "native: {native_out}\n{}", String::from_utf8_lossy(&native.stderr));

    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("app/models/handle.rb", HANDLE)
        .run_ruby(SCRIPT);
    assert!(run.errors.is_empty(), "{:#?}", run.errors);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, native_out, "emitted\n{}", run.stderr);
}

const REGISTRY: &str = r#"module Registry
  SERIALIZE_METHOD = :upcase
  SERIALIZE_PROC = SERIALIZE_METHOD.to_proc
  DIRECT = :downcase.to_proc

  def self.pack(value)
    SERIALIZE_PROC.call(value) + DIRECT.call("X")
  end
end
"#;

/// `Symbol#to_proc` held in a constant (paquito's `SERIALIZE_PROC`) was a
/// value with no inferred type, so each read of it was refused.
#[test]
fn a_constant_holding_symbol_to_proc_is_readable() {
    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("app/models/registry.rb", REGISTRY)
        .run_ruby("p Registry.pack(\"ab\")\n");
    assert!(run.errors.is_empty(), "{:#?}", run.errors);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "\"ABx\"\n", "emitted\n{}", run.stderr);
}
