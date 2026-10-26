//! #1238: a reader whose whole body memoizes a literal into the ivar of
//! its own name (`def label; @label ||= "x"; end`) is emitted as that
//! field, starting at the literal. Before, the field was declared bare
//! (or, static, at its type's zero value) and the method dropped, so
//! every read answered `""`, `undefined` or `[]` where Ruby answers the
//! memo. The run is `typescript_toolchain::real_blog_memoized_fields_run_under_tsx`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/memoized_fields.rs"]
mod memoized_fields;

use roundhouse::project::BuildTarget;

#[test]
fn a_literal_memo_is_the_fields_initial_value() {
    let (emitted, errors) = memoized_fields::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let meter = read("app/models/meter.ts");
    for field in ["static label: string = \"x\";", "name: string = \"n\";", "items: number[] = [1, 2];"] {
        assert!(meter.contains(field), "{field}:\n{meter}");
    }
    // A model's field is otherwise `declare`d (its backing comes from the
    // constructor); a memo's starts at its literal instead.
    let comment = read("app/models/comment.ts");
    assert!(comment.contains("  tone: string = \"calm\";"), "{comment}");
}
