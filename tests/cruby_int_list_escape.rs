//! The CRuby shim's `Db.escape_int_list` (runtime/spinel/db_cruby.rb)
//! renders an all-Integer `IN (...)` list with `JSON.generate`, which
//! writes it in C — two objects for any length — instead of one String
//! per id. Every preload batch is such a list; a campfire room render
//! builds about 160 ids' worth. Lists with anything else keep `to_i`,
//! which is what makes inlining them safe.

use std::path::Path;
use std::process::Command;

#[test]
fn int_lists_render_the_same_rows_without_a_string_per_id() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = r##"
require_relative "runtime/spinel/db_cruby"

raise "empty" unless Db.escape_int_list([]) == "NULL"
raise "ints #{Db.escape_int_list([1, 22, 333]).inspect}" unless Db.escape_int_list([1, 22, 333]) == "1,22,333"
raise "bignum" unless Db.escape_int_list([2**70, -1]) == "1180591620717411303424,-1"
# Not all Integers: each element still goes through to_i.
raise "mixed #{Db.escape_int_list(["4", nil, 5]).inspect}" unless Db.escape_int_list(["4", nil, 5]) == "4, 0, 5"
raise "injection" unless Db.escape_int_list(["1) OR (1=1", 2]) == "1, 2"

# The list selects the same rows either way.
db = SQLite3::Database.new(":memory:")
db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)")
(1..20).each { |i| db.execute("INSERT INTO t VALUES (#{i})") }
got = db.execute("SELECT id FROM t WHERE id IN (#{Db.escape_int_list([3, 7, 19, 40])}) ORDER BY id").flatten
raise "rows #{got.inspect}" unless got == [3, 7, 19]

# One String per id was 53 objects for 50 ids.
ids = (1..50).to_a
3.times { Db.escape_int_list(ids) }
before = GC.stat(:total_allocated_objects)
1000.times { Db.escape_int_list(ids) }
per_call = (GC.stat(:total_allocated_objects) - before) / 1000.0
raise "#{per_call} objects per 50-id list (want <= 4)" unless per_call <= 4
puts "ALL OK"
"##;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(root)
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ALL OK"),
        "int list escape failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    assert!(out.status.success(), "driver exited {:?}", out.status.code());
}
