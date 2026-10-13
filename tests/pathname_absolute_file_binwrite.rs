//! `Pathname#absolute?` / `#relative?` answer a boolean and `File.binwrite`
//! the byte count, and the emitted program answers what Ruby answers.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ADDED: &str = r#"  validates :title, presence: true

  def self.absolute_path?(path) = Pathname.new(path).absolute?
  def self.relative_path?(path) = Pathname.new(path).relative?
  def self.copy(from, to) = File.binwrite(to, File.binread(from))
"#;

const SCRIPT: &str = r#"
require "tmpdir"
raise "absolute?" unless Article.absolute_path?("/tmp/x") && !Article.absolute_path?("tmp/x")
raise "relative?" unless Article.relative_path?("tmp/x") && !Article.relative_path?("/tmp/x")
Dir.mktmpdir do |dir|
  from = File.join(dir, "from.bin")
  to = File.join(dir, "to.bin")
  File.binwrite(from, "\x00\xFFab")
  raise "binwrite count" unless Article.copy(from, to) == 4
  raise "binwrite bytes" unless File.binread(to) == File.binread(from)
end
puts "Pathname and File.binwrite passed"
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit("app/models/article.rb", "  validates :title, presence: true\n", ADDED)
}

#[test]
fn pathname_absolute_and_file_binwrite_execute_after_emission() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn pathname_absolute_and_file_binwrite_execute_in_native_spinel() {
    app().run_spinel(SCRIPT).assert_passes();
}
