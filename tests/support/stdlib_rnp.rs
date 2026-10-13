//! Standard-library constants a production Rails API reaches for, used the
//! way it uses them, shared by the interpreted and native output lanes.
//! The expected values are what plain Ruby answers.

pub const SOURCE: &str = r#"require "fileutils"
require "openssl"
require "csv"
class StdlibProbe
  def self.copy(dir)
    target = File.join(dir, "a", "b")
    FileUtils.mkdir_p(target)
    File.write(File.join(target, "x.txt"), "hi")
    FileUtils.cp(File.join(target, "x.txt"), File.join(target, "y.txt"))
    File.read(File.join(target, "y.txt"))
  end

  def self.collect
    GC.start
    "collected"
  end

  def self.signature
    OpenSSL::HMAC.hexdigest("SHA256", "key", "data")
  end

  def self.signature_size
    OpenSSL::HMAC.digest("SHA256", "key", "data").bytesize
  end

  def self.rows(text)
    CSV.parse(text).length
  rescue CSV::MalformedCSVError
    -1
  end

  def self.pairs
    Enumerator::Product.new([1, 2], [3, 4]).to_a.length
  end

  def self.lazy?(value)
    value.is_a?(Enumerator::Lazy)
  end
end
"#;

/// Everything above, asserted. Run with a scratch directory in `dir`.
pub const ASSERTIONS: &str = r#"
require "tmpdir"
Dir.mktmpdir do |dir|
  raise "copy" unless StdlibProbe.copy(dir) == "hi"
end
raise "gc" unless StdlibProbe.collect == "collected"
raise "hmac #{StdlibProbe.signature}" unless StdlibProbe.signature == "5031fe3d989c6d1537a013fa6e739da23463fdaec3b70137d828e36ace221bd0"
raise "hmac bytes" unless StdlibProbe.signature_size == 32
raise "csv" unless StdlibProbe.rows("a,b\nc,d\n") == 2
raise "csv malformed" unless StdlibProbe.rows("a,\"b") == -1
raise "product" unless StdlibProbe.pairs == 4
raise "lazy" if StdlibProbe.lazy?([1].each)
puts "stdlib constants OK"
"#;

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/services/stdlib_probe.rb", SOURCE)
}
