//! #1236: a model's class method body is typed with the model as its
//! class-side `self`, as the analyzer types a plain class's. Before, the
//! model lowering re-typed it with no `self`, so an implicit-self call
//! (`tag().upcase`) reached the TypeScript emit untyped: `.upcase` was
//! not translated (TS2339, `undefined` at run time) and the method
//! returned `any`. The run is
//! `typescript_toolchain::real_blog_model_class_methods_run_under_tsx`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/model_class_methods.rs"]
mod model_class_methods;

use roundhouse::project::BuildTarget;

#[test]
fn a_model_class_methods_implicit_self_call_is_typed() {
    let (emitted, errors) = model_class_methods::overlay().emit(BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    let comment = std::fs::read_to_string(emitted.join("app/models/comment.ts")).expect("comment.ts");
    assert!(comment.contains("static loud_tag(): string {"), "{comment}");
    assert!(comment.contains("return this.tag().toUpperCase();"), "{comment}");
    assert!(comment.contains("static tag_size(): number {"), "{comment}");
}
