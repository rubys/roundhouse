//! `numericality:` on an attribute no column types: an `attr_accessor` of an
//! `ActiveModel::Model` form, fed request Strings. The attribute keeps what
//! it was given, and Rails' validator parses it (`parse_as_number`), so a
//! String number is a number (activerecord 8.1.4):
//! - `"12"`, `" 12 "`, `"1.5"`, `".5"`, `"5."`, `"1e3"` and `"1_000"` are
//!   numbers; `"12abc"`, `"abc"`, `"0x1A"` and `true` are not;
//! - `only_integer:` reads the raw text: `"7"`, `"+7"` and `"-7"` pass,
//!   while `"7.0"`, `" 7 "` and the Float `7.0` are "must be an integer";
//! - a bound compares the parsed number: `"0"` fails `>= 1`, `" 2 "` passes;
//! - without `allow_*`, nil, `""` and `"  "` are "not a number";
//! - the attribute still reads the String it was given.
//!
//! The ruby family's check read the value as a Numeric, so it rejected
//! `"12"`; with `allow_nil:`/`allow_blank:` the call was refused. It now
//! reads the number the validator parses (`ActiveSupport.numericality_number`).

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const FORM: &str = r#"class SettingsForm
  include ActiveModel::Model
  attr_accessor :amount, :count, :floor, :required

  validates :amount, numericality: true, allow_blank: true
  validates :count, numericality: { only_integer: true }, allow_blank: true
  validates :floor, numericality: { greater_than_or_equal_to: 1 }, allow_nil: true
  validates :required, numericality: { only_integer: true }
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/settings_form.rb", FORM)
}

const TEST: &str = r#"require "test_helper"

class SettingsFormNumericalityTest < ActiveSupport::TestCase
  def errors_for(**attrs)
    form = SettingsForm.new(amount: nil, count: nil, floor: nil, required: 1, **attrs)
    form.valid? ? [] : form.errors.full_messages
  end

  test "a String number is a number" do
    ["12", " 12 ", "1.5", ".5", "5.", "1e3", "1_000"].each do |text|
      assert_equal [], errors_for(amount: text), text
    end
  end

  test "text that is not a number is refused" do
    ["12abc", "abc", "0x1A"].each do |text|
      assert_equal ["Amount is not a number"], errors_for(amount: text), text
    end
    assert_equal ["Amount is not a number"], errors_for(amount: true)
  end

  test "allow_blank skips a blank String" do
    assert_equal [], errors_for(amount: "")
  end

  test "only_integer reads the raw text" do
    ["7", "+7", "-7"].each { |text| assert_equal [], errors_for(count: text), text }
    assert_equal [], errors_for(count: 7)
    ["7.0", " 7 "].each { |text| assert_equal ["Count must be an integer"], errors_for(count: text), text }
    assert_equal ["Count must be an integer"], errors_for(count: 7.0)
  end

  test "a bound compares the parsed number" do
    assert_equal ["Floor must be greater than or equal to 1"], errors_for(floor: "0")
    assert_equal [], errors_for(floor: "1")
    assert_equal [], errors_for(floor: " 2 ")
    assert_equal [], errors_for(floor: "2.5")
  end

  test "without allow_* a blank value is not a number" do
    [nil, "", "  "].each { |value| assert_equal ["Required is not a number"], errors_for(required: value), value.inspect }
  end

  test "the attribute keeps what it was given" do
    form = SettingsForm.new(amount: "12", required: 1)
    form.valid?
    assert_equal "12", form.amount
  end
end
"#;

#[test]
fn an_untyped_attributes_string_is_parsed_as_a_number_on_ruby() {
    app().write("test/models/settings_form_numericality_test.rb", TEST).run_test("test/models/settings_form_numericality_test.rb").assert_passes();
}

/// Warnings `roundhouse --target <target>` prints for the real-blog app with
/// the form added.
fn cli_warnings(target: &str, tag: &str) -> Vec<String> {
    let root = std::env::temp_dir().join(format!("roundhouse-numericality-untyped-{tag}-{target}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    let status = std::process::Command::new("cp")
        .args(["-a"])
        .arg(roundhouse::fixtures::real_blog())
        .arg(&source)
        .status()
        .expect("copy real-blog");
    assert!(status.success());
    std::fs::write(source.join("app/models/settings_form.rb"), FORM).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", target, "--allow-unsupported", "-o"])
        .arg(root.join("out"))
        .arg(&source)
        .output()
        .expect("run roundhouse");
    std::fs::remove_dir_all(&root).ok();
    String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.contains("an untyped attribute's String input")).map(String::from).collect()
}

/// The strict targets don't load the parser, so an `allow_*` line stays the
/// reported gap there; the ruby family reports none.
#[test]
fn only_a_strict_target_keeps_the_allow_gap() {
    let rust = cli_warnings("rust", "allow");
    assert!(rust.iter().any(|w| w.contains("validates :amount, numericality: allow_blank:")), "{rust:#?}");
    assert!(cli_warnings("ruby", "allow").is_empty());
}

const SPINEL_SCRIPT: &str = r##"
def errors_for(**attrs)
  form = SettingsForm.new(amount: nil, count: nil, floor: nil, required: 1, **attrs)
  form.valid? ? [] : form.errors.to_a
end
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
["12", " 12 ", "1.5", ".5", "5.", "1e3", "1_000"].each { |text| check(text, errors_for(amount: text), []) }
["12abc", "abc", "0x1A"].each { |text| check(text, errors_for(amount: text), ["Amount is not a number"]) }
check("blank", errors_for(amount: ""), [])
["7", "+7", "-7"].each { |text| check(text, errors_for(count: text), []) }
["7.0", " 7 "].each { |text| check(text, errors_for(count: text), ["Count must be an integer"]) }
check("floor 0", errors_for(floor: "0"), ["Floor must be greater than or equal to 1"])
check("floor 2", errors_for(floor: " 2 "), [])
check("required nil", errors_for(required: nil), ["Required is not a number"])
check("required spaces", errors_for(required: "  "), ["Required is not a number"])
puts "untyped numericality passed"
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_untyped_attributes_string_is_parsed_as_a_number_on_spinel() {
    let run = app().run_spinel(SPINEL_SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("untyped numericality passed"), "{}", run.stdout);
}

/// Without `allow_*` too: the strict targets' check is `is_a?(Numeric)`,
/// which rejects `"12"`, so the line is the reported gap there rather than
/// a check that refuses what Rails accepts.
#[test]
fn a_strict_target_refuses_an_untyped_numericality_without_allow() {
    let rust = cli_warnings("rust", "no-allow");
    assert!(rust.iter().any(|w| w.contains("validates :required, numericality:")), "{rust:#?}");
}

/// The site build (`project::build_archives`) emits each target from the
/// target's own analysis (`session::analyze_and_lower_for`), as the CLI
/// does, so a strict target's archive refuses the untyped rule the same
/// way: its settings form is the CLI's, file for file.
#[test]
fn the_site_archive_emits_a_strict_target_as_the_cli_does() {
    let root = std::env::temp_dir().join(format!("roundhouse-numericality-untyped-archive-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    std::fs::write(source.join("app/models/settings_form.rb"), FORM).unwrap();
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "rust", "--allow-unsupported", "-o"])
        .arg(root.join("cli"))
        .arg(&source)
        .output()
        .expect("run roundhouse");
    assert!(cli.status.success(), "{}", String::from_utf8_lossy(&cli.stderr));
    roundhouse::project::build_archives(&source, &root.join("site"), &[roundhouse::build_target::BuildTarget::Rust]).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("site/browse/rust.json")).unwrap()).unwrap();
    let files = manifest["files"].as_array().unwrap();
    let form = files
        .iter()
        .find(|f| f["path"].as_str().is_some_and(|p| p.ends_with("settings_form.rs")))
        .expect("settings form in the archive");
    let cli_form = std::fs::read_to_string(root.join("cli").join(form["path"].as_str().unwrap())).unwrap();
    std::fs::remove_dir_all(&root).ok();
    assert_eq!(form["content"].as_str().unwrap(), cli_form);
}

/// The site's archive build and the preview binary print what the
/// target's analysis refused, as the CLI does: a strict target names the
/// untyped `numericality:` rule it dropped.
#[test]
fn archives_and_preview_report_the_refused_rule() {
    let root = std::env::temp_dir().join(format!("roundhouse-numericality-untyped-report-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("app");
    std::fs::create_dir_all(&root).unwrap();
    assert!(std::process::Command::new("cp").args(["-a"]).arg(roundhouse::fixtures::real_blog()).arg(&source).status().unwrap().success());
    std::fs::write(source.join("app/models/settings_form.rb"), FORM).unwrap();
    let archives = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--archives", "rust"])
        .arg(&source)
        .arg("-o")
        .arg(root.join("site"))
        .output()
        .expect("run roundhouse --archives");
    let preview = std::process::Command::new(env!("CARGO_BIN_EXE_emit_preview"))
        .args(["--target", "rs", "--out"])
        .arg(root.join("preview"))
        .arg(&source)
        .output()
        .expect("run emit_preview");
    std::fs::remove_dir_all(&root).ok();
    let archives = String::from_utf8_lossy(&archives.stderr);
    let preview = String::from_utf8_lossy(&preview.stderr);
    assert!(archives.lines().any(|l| l.starts_with("roundhouse: rust:") && l.contains("validates :required, numericality:")), "{archives}");
    assert!(preview.lines().any(|l| l.starts_with("roundhouse:") && l.contains("validates :required, numericality:")), "{preview}");
}
