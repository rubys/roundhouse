//! A control byte in the literal part of an interpolated string is written
//! as an escape, so the emitted file holds no raw NUL or ESC byte and the
//! string keeps its bytes.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ADDED: &str = "  validates :title, presence: true\n\n  def self.tagged(name) = \"#{name}\\0\\e\\x7f\\u0085\"\n";

const SCRIPT: &str = r#"
raise Article.tagged("a").bytes.inspect unless Article.tagged("a").bytes == [97, 0, 27, 127, 194, 133]
puts "control bytes passed"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit("app/models/article.rb", "  validates :title, presence: true\n", ADDED)
}

#[test]
fn control_bytes_in_an_interpolated_string_survive_emission() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let model = std::fs::read(emitted.join("app/models/article.rb")).expect("emitted model");
    assert!(!model.iter().any(|b| matches!(b, 0x00 | 0x1b | 0x7f)), "a raw control byte in the emitted model");
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn control_bytes_in_an_interpolated_string_survive_emission_on_spinel() {
    app().run_spinel(SCRIPT).assert_passes();
}
