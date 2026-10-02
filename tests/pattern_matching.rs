use roundhouse::analyze::{BodyTyper, Ctx};
use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::emit::{diagnostics::scope, ruby::emit_expr};
use roundhouse::expr::Expr;
use roundhouse::ingest::{ingest_app_from_tree, ingest_expr};
use roundhouse::project::{BuildTarget, target_files};
use roundhouse::ty::Ty;
use std::process::Command;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn parse(source: &str) -> Expr {
    let parsed = ruby_prism::parse(source.as_bytes());
    assert_eq!(parsed.errors().count(), 0, "invalid test source: {source}");
    let program = parsed.node();
    ingest_expr(
        &program.as_program_node().unwrap().statements().as_node(),
        "pattern.rb",
    )
    .unwrap()
}

#[test]
fn native_matches_preserve_expression_precedence_and_pattern_syntax() {
    for (source, expected) in [
        ("result = ((true || \"x\") in Integer)", "false"),
        ("(nil || 7) => n; result = n", "7"),
        (
            "result = [((1 + 2) in 3), ((1 in Integer).to_s)]",
            "[true, \"true\"]",
        ),
        (
            "result = (case {\"hyphen-key\": 7}; in {\"hyphen-key\": n}; n; end)",
            "7",
        ),
        ("result = (case 2; in (1 | 2) => n; n; end)", "2"),
        ("result = (case 7; in (Integer => _n) | 1; _n; end)", "7"),
        ("result = (nil in ^(nil))", "true"),
        ("result = (\"ab\" in /a/)", "true"),
        ("result = (if 7 in n; n; end)", "7"),
        ("result = (case {a: 7}; in {**}; 13; end)", "13"),
        ("result = (case {a: 7}; in {}; 13; else; 19; end)", "19"),
        (
            "counter = 0; matched = (1 in ^(counter += 1)); result = [matched, counter]",
            "[true, 1]",
        ),
    ] {
        let emitted = emit_expr(&parse(source));
        let output = Command::new("ruby")
            .args(["-e", &format!("{emitted}\np result")])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{source}\n{emitted}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            expected,
            "{source}\n{emitted}"
        );
        let round_trip = Command::new(env!("CARGO_BIN_EXE_roundhouse-ast"))
            .args(["--round-trip", "-e", source])
            .output()
            .unwrap();
        assert!(
            round_trip.status.success(),
            "{source}\n{}\n{}",
            String::from_utf8_lossy(&round_trip.stdout),
            String::from_utf8_lossy(&round_trip.stderr)
        );
    }
}

#[test]
fn pattern_locals_survive_guard_failure_predicates_and_required_matches() {
    let classes = Default::default();
    let typer = BodyTyper::new(&classes);
    for (source, expected) in [
        (
            "case 7; in n if false; in Integer; end; n",
            Ty::Union {
                variants: vec![Ty::Int, Ty::Nil],
            },
        ),
        (
            "case 7; in n if false; in Integer; n; end",
            Ty::Union {
                variants: vec![Ty::Int, Ty::Nil],
            },
        ),
        (
            "7 in n; n",
            Ty::Union {
                variants: vec![Ty::Int, Ty::Nil],
            },
        ),
        ("7 => n; n", Ty::Int),
        (
            "if 7 in n; n; else; nil; end",
            Ty::Union {
                variants: vec![Ty::Int, Ty::Nil],
            },
        ),
        (
            "(7 in n) && n",
            Ty::Union {
                variants: vec![Ty::Bool, Ty::Int, Ty::Nil],
            },
        ),
        (
            "7 => ((Integer => _n) | 1); _n",
            Ty::Union {
                variants: vec![Ty::Int, Ty::Nil],
            },
        ),
    ] {
        let mut expr = parse(source);
        assert_eq!(
            typer.analyze_expr(&mut expr, &Ctx::default()),
            expected,
            "{source}"
        );
    }
}

#[test]
fn unsupported_targets_reject_even_the_apparently_simple_subset() {
    for (body, construct) in [
        ("case 1; in nil; 0; in 1; 7; end", "CaseMatch"),
        ("case 5; in n; n + 1; end", "CaseMatch"),
        ("case 5; in Integer; 7; else; 9; end", "CaseMatch"),
        ("case 5; in 1..8; 7; else; 9; end", "CaseMatch"),
        ("case 5; in 1 | 5; 7; else; 9; end", "CaseMatch"),
        ("5 in Integer", "MatchPredicate"),
        ("5 => n", "MatchRequired"),
    ] {
        let source = format!("class Matcher\n  def self.probe\n    {body}\n  end\nend\n");
        let tree = [("app/lib/matcher.rb".into(), source.into_bytes())]
            .into_iter()
            .collect();
        let app = ingest_app_from_tree(tree).unwrap();
        for &target in BuildTarget::TRANSPILE {
            if matches!(
                target,
                BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel | BuildTarget::Roda
            ) {
                continue;
            }
            let (files, diagnostics) =
                scope(|| target_files(&app, std::path::Path::new("not-a-fixture"), target));
            assert!(files.is_err(), "{target:?} accepted {body}");
            assert_eq!(diagnostics.len(), 1, "{target:?}: {diagnostics:?}");
            assert_eq!(diagnostics[0].severity, Severity::Error);
            assert!(
                matches!(&diagnostics[0].kind, DiagnosticKind::Unsupported { construct: c, target: Some(t), .. }
                if c.as_str() == construct && t.as_str() == target.as_str()),
                "{target:?}: {diagnostics:?}"
            );
            assert!(!diagnostics[0].span.is_synthetic(), "lost source span");
        }
    }
}

#[test]
fn unsupported_matches_in_defaults_and_fixtures_are_not_missed() {
    for (path, source) in [
        (
            "app/controllers/matchers_controller.rb",
            "class MatchersController < ActionController::Base\n  def index(matched: (5 in Integer))\n    matched\n  end\nend\n",
        ),
        (
            "app/models/matcher.rb",
            "class Matcher < ApplicationRecord\n  belongs_to :owner, default: -> { 5 in Integer }\nend\n",
        ),
        (
            "app/models/matcher.rb",
            "class Matcher < ApplicationRecord\n  has_many :items, -> { 5 in Integer }\nend\n",
        ),
        (
            "test/fixtures/matchers.yml",
            "one:\n  matched: <%= 5 in Integer %>\n",
        ),
        (
            "test/fixtures/matchers.yml",
            "<% 5 => n %>\none:\n  number: 7\n",
        ),
    ] {
        let tree = [(path.into(), source.as_bytes().to_vec())]
            .into_iter()
            .collect();
        let app = ingest_app_from_tree(tree).unwrap();
        let (files, diagnostics) = scope(|| {
            target_files(
                &app,
                std::path::Path::new("not-a-fixture"),
                BuildTarget::Rust,
            )
        });
        assert!(files.is_err(), "accepted {path}: {source}");
        assert_eq!(diagnostics.len(), 1, "{path}: {diagnostics:?}");
        assert_eq!(diagnostics[0].severity, Severity::Error);
        assert!(
            matches!(&diagnostics[0].kind, DiagnosticKind::Unsupported { construct, target: Some(t), .. }
                if (construct.as_str() == "MatchPredicate" || construct.as_str() == "MatchRequired") && t.as_str() == "rust"),
            "{path}: {diagnostics:?}"
        );
        assert!(!diagnostics[0].span.is_synthetic());
    }
}

#[test]
fn pattern_visitors_cover_pins_constants_guards_and_bodies() {
    fn sends(expr: &Expr, names: &mut Vec<String>) {
        if let roundhouse::expr::ExprNode::Send { method, .. } = &*expr.node {
            names.push(method.to_string());
        }
        if let roundhouse::expr::ExprNode::Const { path } = &*expr.node {
            names.extend(path.iter().map(ToString::to_string));
        }
        expr.node.for_each_child(&mut |child| sends(child, names));
    }
    fn rename(expr: &mut Expr) {
        if let roundhouse::expr::ExprNode::Send { method, .. } = &mut *expr.node {
            *method = roundhouse::Symbol::from(format!("{method}_visited"));
        }
        if let roundhouse::expr::ExprNode::Const { path } = &mut *expr.node {
            for name in path {
                *name = roundhouse::Symbol::from(format!("{name}_visited"));
            }
        }
        expr.node.for_each_child_mut(&mut rename);
    }
    let mut expr = parse(
        "case subject; in Container[{a: ^(first)}, ^(second)] if guard; body; in [*, ^(third), *]; other; else; fallback; end",
    );
    let expected = [
        "subject",
        "Container",
        "first",
        "second",
        "guard",
        "body",
        "third",
        "other",
        "fallback",
    ];
    let mut names = Vec::new();
    sends(&expr, &mut names);
    assert_eq!(names, expected);
    rename(&mut expr);
    names.clear();
    sends(&expr, &mut names);
    assert_eq!(names, expected.map(|name| format!("{name}_visited")));
}

#[test]
fn structural_matching_runs_in_an_emitted_application() {
    emit_and_run::real_blog()
        .write(
            "app/lib/structural_matcher.rb",
            r#"class StructuralMatcher
  def self.required(value)
    value => [a, b, *tail]
    [a, b, tail]
  end
  def self.predicate(value)
    matched = (value in {status: "ok", data:})
    [matched, data]
  end
  def self.guarded(value)
    case value
    in n if false
      91
    in Integer unless false
      n
    else
      -17
    end
  end
  def self.dispatch(value)
    case value
    in [Integer => n, *rest, String => last]
      [n, rest, last]
    in {a:, **nil}
      a
    in [*, 13, *after]
      after
    end
  end
  def self.ignore_keys(value)
    case value
    in {**}
      41
    else
      -2
    end
  end
end
"#,
        )
        .run_ruby(
            r#"
def same(expected, actual)
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
end
same([3, 7, [11, 19]], StructuralMatcher.required([3, 7, 11, 19]))
same([true, 23], StructuralMatcher.predicate({status: "ok", data: 23}))
same([false, nil], StructuralMatcher.predicate({status: "no"}))
same(31, StructuralMatcher.guarded(31))
same(-17, StructuralMatcher.guarded("not an integer"))
same([5, [7, 11], "end"], StructuralMatcher.dispatch([5, 7, 11, "end"]))
same(29, StructuralMatcher.dispatch({a: 29}))
same([17, 19], StructuralMatcher.dispatch([2, 3, 13, 17, 19]))
same(41, StructuralMatcher.ignore_keys({a: 29, extra: 1}))
same(-2, StructuralMatcher.ignore_keys([29]))
begin
  StructuralMatcher.required([1])
  raise "required mismatch did not raise"
rescue NoMatchingPatternError
end
begin
  StructuralMatcher.dispatch({a: 29, extra: 1})
  raise "**nil mismatch did not raise"
rescue NoMatchingPatternError
end
puts "structural matching checks passed"
"#,
        )
        .assert_passes();
}
