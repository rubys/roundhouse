//! `recv&.attr ||= v` (and `&&=`, `+=`) on the CRuby tree, against the same
//! files run natively. Ruby evaluates the receiver once and, when it is nil,
//! skips the read, the operator, the value and the write: the whole
//! expression is nil. The ingest read the attribute write as a plain
//! `recv.attr ||= v`, so the guard was gone and a nil receiver raised
//! NoMethodError. A `request.details&.profile_id ||= …` hit it on every request
//! that carries no details.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

const SOURCE: &str = r##"class Details
  attr_accessor :profile_id, :count, :flag

  def initialize
    @count = 1
    @flag = true
  end
end

class Holder
  attr_reader :reads

  def initialize(details)
    @details = details
    @reads = 0
  end

  def details
    @reads += 1
    @details
  end

  def or_assign(value)
    details&.profile_id ||= value
  end

  def and_assign(value)
    details&.flag &&= value
  end

  def add(n)
    details&.count += n
  end

  def local_or(local, value)
    local&.profile_id ||= value
  end

  # `&.` skips only nil: a false receiver is read, as Ruby reads it.
  def false_receiver
    false&.to_s ||= "x"
  end
end
"##;

const SCRIPT: &str = r##"def seen(label)
  value = begin
    yield
  rescue StandardError => e
    [e.class.name, e.message]
  end
  puts "#{label}: #{value.inspect}"
end
nothing = Holder.new(nil)
seen("or nil") { nothing.or_assign(7) }
seen("and nil") { nothing.and_assign(false) }
seen("add nil") { nothing.add(2) }
seen("local nil") { nothing.local_or(nil, 3) }
seen("reads") { nothing.reads }
details = Details.new
holder = Holder.new(details)
seen("or set") { holder.or_assign(7) }
seen("or kept") { holder.or_assign(9) }
seen("profile") { details.profile_id }
seen("and set") { holder.and_assign(false) }
seen("flag") { details.flag }
seen("add") { holder.add(4) }
seen("count") { details.count }
seen("local set") { holder.local_or(Details.new, 5) }
seen("reads once") { holder.reads }
seen("false receiver") { holder.false_receiver }
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("app/.keep", "")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("lib/safe_navigation_probe.rb", SOURCE)
}

fn native() -> String {
    let dir = std::env::temp_dir().join(format!("rh_safe_nav_op_assign_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("probe.rb"), format!("{SOURCE}\n{SCRIPT}")).unwrap();
    let out = Command::new("ruby").arg("probe.rb").current_dir(&dir).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native run failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout
}

#[test]
fn a_safe_navigation_op_assign_skips_a_nil_receiver() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, native(), "emitted safe-navigation op-assign vs native Ruby");
}
