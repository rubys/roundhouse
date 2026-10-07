//! JDBC setter contract; the CRuby/Spinel runtime proof lives in param_binds.

use std::process::Command;

#[test]
fn jruby_boolean_binding_distinguishes_null_from_false() {
    let script = r#"
src = File.read("runtime/spinel/db_jruby.rb")
defn = src[/^  def self\.bind_bool\(stmt, idx, value\)\n.*?^  end\n/m] or abort "no bind_bool"
module Db; end
module Java
  module JavaSql
    module Types
      INTEGER = 4
    end
  end
end
Db.module_eval(defn)
class JdbcBoolRecorder
  attr_reader :call
  def set_null(index, type); @call = [:null, index, type]; end
  def set_int(index, value); @call = [:int, index, value]; end
end
handle = Struct.new(:pstmt).new(JdbcBoolRecorder.new)
[[false, [:int, 3, 0]], [nil, [:null, 3, 4]], [true, [:int, 3, 1]],
 [nil, [:null, 3, 4]], [false, [:int, 3, 0]]].each do |value, expected|
  Db.bind_bool(handle, 3, value)
  abort handle.pstmt.call.inspect unless handle.pstmt.call == expected
end
Db.bind_bool(Struct.new(:pstmt).new(nil), 1, nil)
puts "JRuby nullable boolean setter contract passed (no JDBC runtime)"
"#;
    let output = Command::new("ruby")
        .args(["-e", script])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("ruby");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
