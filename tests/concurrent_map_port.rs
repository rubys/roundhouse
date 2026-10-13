//! Pin the Spinel `Concurrent::Map` port under CRuby without the gem.
//!
//! Default `emit_and_run` exercises the ruby-family gem swap. The new
//! executable logic is `runtime/spinel/concurrent.rb`; load it alone and
//! assert `[]` / `[]=` / `compute_if_absent` / `clear` (including nil
//! present and raise-during-compute leaving the key absent).

use std::process::Command;

#[test]
fn spinel_concurrent_map_port_runs_under_cruby() {
    let script = r#"
raise "gem Concurrent already loaded" if defined?(Concurrent)
load "runtime/spinel/concurrent.rb"
m = Concurrent::Map.new
m[:a] = 1
raise "[]" unless m[:a] == 1
raise "[]= answers value" unless (m[:b] = 2) == 2
v = m.compute_if_absent(:c) { 3 }
raise "compute_if_absent" unless v == 3 && m[:c] == 3
m[:n] = nil
raise "nil counts as present" unless m.compute_if_absent(:n) { 9 }.nil?
raised = false
begin
  m.compute_if_absent(:boom) { raise "x" }
rescue RuntimeError
  raised = true
end
raise "raise during compute" unless raised && m[:boom].nil?
c = m.clear
raise "clear" unless c.equal?(m) && m[:a].nil?
print "OK"
"#;
    let out = Command::new("ruby")
        .arg("-e")
        .arg(script)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "OK",
        "stdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
}
