//! `Error = Class.new(StandardError)` in a class body defines a class, as
//! `class Error < StandardError; end` does (maybe and discourse write
//! their error classes this way); a bare parent naming an earlier one of
//! the same owner is that one. At file level it defines the top-level
//! class, read from other files, and a block of `def`s gives the class
//! those methods.

use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/error.rb", "class Error < StandardError\nend\n")
        .write(
            "app/models/importer.rb",
            "class Importer\n  Error = Class.new(StandardError)\n  TooBig = Class.new(Error)\n  Rooted = Class.new(::Error)\n\n  def self.run(size)\n    raise TooBig, \"too big: #{size}\" if size > 10\n    raise Error, \"empty\" if size.zero?\n    \"ok\"\n  rescue TooBig => e\n    \"too big caught: #{e.message}\"\n  rescue Error => e\n    \"error caught: #{e.message}\"\n  end\n\n  def self.lineage\n    [TooBig.superclass.name, Error.superclass.name, Rooted.superclass.name]\n  end\nend\n",
        )
}

const ASSERTIONS: &str = r##"got = [Importer.run(5), Importer.run(11), Importer.run(0), Importer.lineage]
raise got.inspect unless got == ["ok", "too big caught: too big: 11", "error caught: empty", ["Importer::Error", "StandardError", "Error"]]
puts "class new passed"
"##;

#[test]
fn a_class_new_constant_is_a_class() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_class_new_constant_is_a_class_on_spinel() {
    app().run_spinel(ASSERTIONS).assert_passes();
}

fn file_level_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/limit_errors.rb", LIMIT_ERRORS)
        .write("app/models/limiter.rb", LIMITER)
}

const LIMIT_ERRORS: &str = r##"BaseError = Class.new(StandardError)
LimitError = Class.new(BaseError) do
  attr_reader :size

  def initialize(size)
    @size = size
    super("too many: #{size}")
  end

  def code = status

  private

  def status = 429
end
"##;

const LIMITER: &str = r##"TIMEOUT = 5
DeniedError = Class.new(BaseError)

class Limiter
  Nested = Class.new(StandardError) do
    attr_reader :code

    def initialize(message, code = 7)
      @code = code
      super(message)
    end
  end

  def self.check(size)
    raise LimitError.new(size) if size > 3
    raise DeniedError, "denied" if size.zero?
    raise Nested, "nested" if size == 1
    "ok"
  rescue LimitError => e
    "#{e.code} #{e.message} #{e.size}"
  rescue Nested => e
    "#{e.code} #{e.message}"
  rescue BaseError => e
    "base #{e.message}"
  end

  def self.lineage
    [LimitError.superclass.name, DeniedError.superclass.name, Nested.name, TIMEOUT, LimitError.private_method_defined?(:status)]
  end
end
"##;

const FILE_LEVEL_ASSERTIONS: &str = r##"got = [Limiter.check(2), Limiter.check(5), Limiter.check(0), Limiter.check(1), Limiter.lineage]
raise got.inspect unless got == ["ok", "429 too many: 5 5", "base denied", "7 nested", ["BaseError", "BaseError", "Limiter::Nested", 5, true]]
puts "file-level class new passed"
"##;

#[test]
fn a_file_level_class_new_constant_is_a_top_level_class() {
    file_level_app().run_ruby(FILE_LEVEL_ASSERTIONS).assert_passes();
}
