//! A global-variable write in a carried method body runs as Ruby runs it.
//!
//! Reduction: a method that restores `$stdout` / `$stderr` from the values
//! it saved before capturing them. The expression ingest refused a global
//! write, so the tree carried a bare `nil` in its place. A global write
//! is an `LValue::Var` with its `$` name, as a read is.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LIB: &str = r#"require "stringio"

class StdioRestore
  def self.run
    out = StringIO.new
    saved_out = $stdout
    saved_err = $stderr
    $stdout = out
    $stderr = out
    puts "captured"
    warn "warned"
    $stdout = saved_out
    $stderr = saved_err
    [out.string, $stdout.equal?(saved_out), $stderr.equal?(saved_err), $stdout.equal?(out)]
  end

  def self.memo
    $rh_memo ||= []
    $rh_memo << :hit
    $rh_count ||= 0
    $rh_count += 1
    $rh_gate &&= :never
    [$rh_memo, $rh_count, $rh_gate]
  end

  def self.in_block
    [1, 2].each { |i| $rh_last = i * 10 }
    $rh_last
  end
end
"#;

const SCRIPT: &str = "p StdioRestore.run\np StdioRestore.memo\np StdioRestore.memo\np StdioRestore.in_block\n";

#[test]
fn global_variable_writes_in_a_method_run_as_natively() {
    let dir = std::env::temp_dir().join(format!("rh_gvar_native_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("stdio_restore.rb"), LIB).unwrap();
    let native = emit_and_run::ruby()
        .arg("-I").arg(&dir).arg("-rstdio_restore").arg("-e").arg(SCRIPT)
        .output()
        .expect("ruby");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let native = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(
        native,
        "[\"captured\\nwarned\\n\", true, true, false]\n[[:hit], 1, nil]\n[[:hit, :hit], 2, nil]\n20\n"
    );
    let run = emit_and_run::empty_app()
        .write("config/environments/development.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("lib/stdio_restore.rb", LIB)
        .run_ruby(SCRIPT);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, native);
}
