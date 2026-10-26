use super::emit_and_run;

/// An operator whose receiver is a nullable column: Ruby operates on a
/// present value and raises NoMethodError on nil. These were
/// IncompatibleBinop errors; the strict targets now bind the receiver,
/// raise on nil and operate on the present value. Answers are Ruby
/// 3.4's for the same model and records.
fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "t.text \"body\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"", "t.text \"body\"\n    t.integer \"rank\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"")
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def early?
    rank < 3
  end

  def next_rank
    rank + 1
  end

  def shout
    title + \"!\"
  end
",
        )
}

#[test]
fn nil_receiver_operators_run_like_ruby() {
    let script = r#"present = Article.new(title: "t", body: "A sufficiently long body.", rank: 1)
raise "early" unless present.early? == true
raise "next_rank" unless present.next_rank == 2
raise "shout" unless present.shout == "t!"
raise "empty title" unless Article.new(title: "", rank: 1).shout == "!"
absent = Article.new(body: "A sufficiently long body.")
got = [:early?, :next_rank, :shout].map do |m|
  absent.public_send(m)
  "none"
rescue NoMethodError => e
  e.message
end
want = ["undefined method '<' for nil", "undefined method '+' for nil", "undefined method '+' for nil"]
raise "absent: #{got.inspect}" unless got == want
puts "nil operand passed"
"#;
    let run = app().run_ruby(script);
    run.assert_passes();
    assert!(run.stdout.contains("nil operand passed"));
}

/// Each strict target binds the receiver once and raises before the
/// operator reads it.
#[test]
fn strict_targets_raise_on_a_nil_receiver() {
    use roundhouse::project::BuildTarget;
    let cases: &[(BuildTarget, &str, &str)] = &[
        (BuildTarget::Rust, "src/models/article.rs", "raise_class(\"NoMethodError\", (\"undefined method '<' for nil\")"),
        (BuildTarget::Go, "app/v2/article.go", "return (*nil_operand_0) < 3"),
        (BuildTarget::Typescript, "app/models/article.ts", "const nil_operand_0 = this.rank;"),
        (BuildTarget::Python, "app/v2/models.py", "(lambda nil_operand_0: "),
        (BuildTarget::Kotlin, "src/main/kotlin/app/models/Article.kt", "throw NoMethodError(\"undefined method '<' for nil\")"),
        (BuildTarget::Swift, "Sources/App/app/models/Article.swift", "throw NoMethodError(\"undefined method '<' for nil\")"),
        (BuildTarget::CSharp, "app/models/Article.cs", "? throw new NoMethodError(\"undefined method '<' for nil\") :"),
        (BuildTarget::Crystal, "src/models/article.cr", "raise NoMethodError.new(\"undefined method '<' for nil\")"),
        (BuildTarget::Elixir, "lib/article.ex", "raise \"undefined method '<' for nil\""),
    ];
    for (target, path, needle) in cases {
        let (emitted, errors) = app().emit(*target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
        let source = std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{target:?} {path}: {e}"));
        assert!(source.contains(needle), "{target:?}: {needle:?} in {path}:\n{source}");
    }
}

#[test]
#[ignore = "requires the Rust toolchain; compiles and executes an emitted app"]
fn nil_receiver_operators_run_on_rust() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::create_dir_all(emitted.join("tests")).unwrap();
    std::fs::write(emitted.join("tests/nil_operand.rs"), r#"
use app::models::Article;

fn attrs(pairs: &[(&str, serde_json::Value)]) -> std::collections::HashMap<String, serde_json::Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

#[test]
fn a_nil_receiver_raises_and_a_present_one_operates() {
    let present = Article::new(attrs(&[("title", "t".into()), ("rank", 1_i64.into())]));
    assert!(present.early_pred());
    assert_eq!(present.next_rank(), 2);
    assert_eq!(present.shout(), "t!");
    let absent = Article::new(attrs(&[]));
    let raised = |f: &dyn Fn()| {
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("no raise");
        let e = app::errors_ext::Rescued::from_panic(&*panic);
        assert!(e.is_a("NoMethodError"), "{}", e.class);
        e.message
    };
    assert_eq!(raised(&|| { absent.early_pred(); }), "undefined method '<' for nil");
    assert_eq!(raised(&|| { absent.next_rank(); }), "undefined method '+' for nil");
    assert_eq!(raised(&|| { absent.shout(); }), "undefined method '+' for nil");
}
"#).unwrap();
    let output = std::process::Command::new("cargo")
        .args(["test", "--test", "nil_operand"])
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(&*emitted)
        .output()
        .expect("run the emitted Rust project's test");
    assert!(output.status.success(), "{}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
