//! `begin … end until cond` / `begin … end while cond` run the body
//! before the first test.
//!
//! A retry loop written `begin … end until state != :error || attempt >= 3`
//! was refused by ingest, and the walk dropped the whole file. Native Ruby
//! on the same source is the oracle.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const RETRY: &str = r#"module Retry
  def self.attempts(limit)
    attempt = 0
    state = :error
    begin
      attempt += 1
      state = attempt >= limit ? :ok : :error
    end until state != :error || attempt >= 3
    [attempt, state]
  end

  def self.runs_once_when_false
    n = 0
    begin
      n += 1
    end while false
    n
  end

  def self.counts_up(to)
    n = 0
    begin
      n += 1
    end while n < to
    n
  end

  def self.keeps_the_scope
    begin
      seen = 5
    end until true
    seen
  end

  def self.breaks_out
    n = 0
    begin
      n += 1
      break if n == 2
    end while true
    n
  end

  def self.value_of_the_loop
    r = (begin; 1; end until true)
    r.inspect
  end

  def self.inner_block_next
    out = []
    i = 0
    begin
      [1, 2, 3].each { |x| next if x == 2; out << x * 10 + i }
      i += 1
    end while i < 2
    out
  end
end
"#;

const REPORT: &str = r#"p Retry.attempts(2)
p Retry.attempts(10)
p Retry.runs_once_when_false
p Retry.counts_up(4)
p Retry.counts_up(0)
p Retry.keeps_the_scope
p Retry.breaks_out
p Retry.value_of_the_loop
p Retry.inner_block_next
"#;

#[test]
fn emitted_do_while_matches_native_ruby() {
    let dir = std::env::temp_dir().join(format!("roundhouse-do-while-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("retry.rb");
    std::fs::write(&file, RETRY).unwrap();
    let native = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("require {:?}\n{REPORT}", file.display().to_string()))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert!(expected.starts_with("[2, :ok]\n[3, :error]\n1\n4\n1\n5\n2\n"), "native control: {expected}");

    let (emitted, errors) = emit_and_run::real_blog().write("app/lib/retry.rb", RETRY).emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let main = emitted.join("main").display().to_string();
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require {main:?}\nMain.configure_default_adapter!\n{REPORT}"))
        .current_dir(&emitted)
        .env("BLOG_DB", ":memory:")
        .output()
        .expect("spawn ruby");
    assert!(
        output.status.success(),
        "emitted program failed in {}:\n{}",
        emitted.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn a_next_aimed_at_the_loop_is_surveyed_as_unsupported() {
    let root = std::env::temp_dir().join(format!("roundhouse-do-while-next-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        (
            "app/lib/skipper.rb",
            "module Skipper\n  def self.run\n    i = 0\n    begin\n      i += 1\n      next if i == 1\n    end while i < 3\n    i\n  end\nend\n",
        ),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg("--continue").arg(&root).output().unwrap();
    let _ = std::fs::remove_dir_all(&root);
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("`next` in a `begin … end while` body"), "{text}");
    assert!(text.contains("app/lib/skipper.rb"), "{text}");
}
