//! Native `case … in` retains Ruby matching semantics through IR and emission.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emit(body: &str) -> String {
    let files: Vec<(&str, String)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\" do |t|\n    t.string \"name\"\n  end\nend\n".to_string(),
        ),
        ("app/helpers/matcher.rb", body.to_string()),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("a `case/in` must ingest");
    let out = ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("matcher.rb"))
        .map(|f| f.content.clone())
        .expect("matcher.rb");
    let errors: Vec<String> =
        ruby_prism::parse(out.as_bytes()).errors().map(|e| String::from_utf8_lossy(e.message()).into_owned()).collect();
    assert!(errors.is_empty(), "emitted Ruby does not parse: {errors:?}\n{out}");
    out
}

fn assert_runs(out: &str, assertions: &str) {
    let script = format!("{out}\n{assertions}");
    let run = std::process::Command::new("ruby").args(["-e", &script]).output().unwrap();
    assert!(run.status.success(), "{}\n{script}", String::from_utf8_lossy(&run.stderr));
}

#[test]
fn an_array_pattern_with_alternatives_remains_native() {
    let out = emit(
        r#"class Matcher
  def check(level, parent)
    case [level, parent]
    in Ad, Ad | nil
      1
    in AdGroup, Campaign
      2
    else
      3
    end
  end
end
"#,
    );
    assert_runs(&out, "class Ad; end; class AdGroup; end; class Campaign; end; raise unless Matcher.new.check(Ad.new, nil) == 1; raise unless Matcher.new.check(AdGroup.new, Campaign.new) == 2; raise unless Matcher.new.check(nil, nil) == 3");
    assert!(out.contains("case [level, parent]") && out.contains("in [Ad, (Ad | nil)]") && out.contains("else"), "got:\n{out}");
}

#[test]
fn without_an_else_a_miss_raises_no_matching_pattern_error() {
    let out = emit(
        r#"class Matcher
  def check(x)
    case x
    in Integer
      1
    end
  end
end
"#,
    );
    assert_runs(&out, "raise unless Matcher.new.check(3) == 1; begin; Matcher.new.check(nil); raise 'miss accepted'; rescue NoMatchingPatternError; end");
    assert!(out.contains("in Integer") && !out.contains("else"), "native matching raises on a miss:\n{out}");
}

#[test]
fn captures_and_binds_are_assigned_before_the_body() {
    let out = emit(
        r#"class Matcher
  def check(x)
    case x
    in [Integer => id, name]
      [id, name]
    else
      nil
    end
  end
end
"#,
    );
    assert_runs(&out, "raise unless Matcher.new.check([3, :name]) == [3, :name]; raise unless Matcher.new.check([nil, :name]).nil?");
    assert!(out.contains("in [(Integer => id), name]") && out.contains("[id, name]"), "got:\n{out}");
}

#[test]
fn a_splat_binds_the_slice_between_the_fixed_elements() {
    let out = emit(
        r#"class Matcher
  def check(x)
    case x
    in [first, *middle, last]
      middle
    else
      nil
    end
  end
end
"#,
    );
    assert_runs(&out, "raise unless Matcher.new.check([1, 2, 3, 4]) == [2, 3]; raise unless Matcher.new.check([1]).nil?");
    assert!(out.contains("in [first, *middle, last]"), "got:\n{out}");
}

#[test]
fn a_hash_pattern_requires_its_keys_and_binds_the_shorthand() {
    let out = emit(
        r#"class Matcher
  def check(x)
    case x
    in { type: :card, amount: }
      amount
    else
      0
    end
  end
end
"#,
    );
    assert_runs(&out, "raise unless Matcher.new.check({type: :card, amount: 7}) == 7; raise unless Matcher.new.check({type: :card}) == 0");
    assert!(out.contains("in { type: :card, amount: }"), "got:\n{out}");
}

#[test]
fn a_guard_sees_the_bindings_and_a_failed_guard_falls_through() {
    let out = emit(
        r#"class Matcher
  def check(x)
    case x
    in [n] if n > 1
      :big
    in [_]
      :small
    else
      :none
    end
  end
end
"#,
    );
    assert_runs(&out, "raise unless Matcher.new.check([3]) == :big; raise unless Matcher.new.check([1]) == :small; raise unless Matcher.new.check([]) == :none");
    assert!(out.contains("in [n] if n > 1"), "got:\n{out}");
    assert!(out.contains("n > 1"), "got:\n{out}");
    assert!(out.contains(":big") && out.contains(":small") && out.contains(":none"), "got:\n{out}");
}

#[test]
fn a_pin_compares_with_case_equality() {
    let out = emit(
        r#"class Matcher
  def check(x, expected)
    case x
    in ^expected
      1
    else
      2
    end
  end
end
"#,
    );
    assert_runs(&out, "raise unless Matcher.new.check(3, 3) == 1; raise unless Matcher.new.check(3, 4) == 2");
    assert!(out.contains("in ^expected"), "got:\n{out}");
}
