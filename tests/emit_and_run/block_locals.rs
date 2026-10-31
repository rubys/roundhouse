//! A block-local (`|n; tag|`) is the block's own variable: writing it
//! leaves the enclosing method's `tag` alone, and every call starts it
//! at nil (#687). Dropped at ingest, the block's write reassigned the
//! outer local, with no diagnostic.

use super::emit_and_run;

#[test]
fn a_block_local_shadows_the_enclosing_local() {
    const SOURCE: &str = r##"class BlockLocalProbe
  def self.write
    tag = "outer"
    [1, 2].each { |n; tag| tag = n.to_s }
    tag
  end
  def self.read_first
    tag = "outer"
    seen = []
    [1, 2].each { |n; tag| seen << tag.inspect; tag = n }
    [tag, seen]
  end
  def self.nested
    tag = "outer"
    inner = nil
    [1].each do |n; tag|
      tag = "mid"
      [2].each { |m| tag = "#{tag}+#{m}" }
      inner = tag
    end
    [tag, inner]
  end
  def self.shadowed_by_a_param
    tag = "outer"
    [1].each do |n; tag|
      tag = "mid"
      [2].each { |tag| tag = "param" }
    end
    tag
  end
  def self.rescue_binding
    err = "outer"
    [1].each do |n; err|
      begin
        raise "boom"
      rescue => err
        err.message
      end
    end
    err
  end
  def self.pattern_binding
    name = "outer"
    got = nil
    [{ name: "y" }].each do |h; name|
      case h
      in { name: }
        got = name
      end
    end
    [name, got]
  end
  def self.lambda_literal
    tag = "outer"
    set = ->(n; tag) { tag = n }
    set.call(9)
    tag
  end
  def self.shadowed_by_a_post_param
    tag = "outer"
    inner = nil
    [1].each do |n; tag|
      tag = "mid"
      [[1, 2]].each { |*rest, tag| tag }
      inner = tag
    end
    [tag, inner]
  end
  def self.shadowed_by_a_destructured_param
    tag = "outer"
    inner = nil
    [1].each do |n; tag|
      tag = "mid"
      [[1, 2]].each { |(a, tag)| tag }
      inner = tag
    end
    [tag, inner]
  end
end
"##;
    const ASSERTIONS: &str = r##"
expected = {write: "outer", read_first: ["outer", ["nil", "nil"]], nested: ["outer", "mid+2"], shadowed_by_a_param: "outer", rescue_binding: "outer", pattern_binding: ["outer", "y"], lambda_literal: "outer", shadowed_by_a_destructured_param: ["outer", "mid"], shadowed_by_a_post_param: ["outer", "mid"]}
expected.each do |method, want|
  got = BlockLocalProbe.public_send(method)
  raise "#{method}: #{got.inspect}, expected #{want.inspect}" unless got == want
end
"##;
    let native = std::process::Command::new("ruby").arg("-e")
        .arg(format!("{SOURCE}\n{ASSERTIONS}"))
        .output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    emit_and_run::real_blog()
        .write("app/services/block_local_probe.rb", SOURCE)
        .run_ruby(ASSERTIONS).assert_passes();
}
