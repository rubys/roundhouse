//! RBS type forms the reader used to refuse, taking the whole method
//! signature with them: `singleton(Foo)`, `bot`, `top`, a method-level
//! type variable, `A & B` and literal types.

use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ty::Ty;

/// The one method `m` of `class C`, as `(param types, return type)`.
fn read(signature: &str) -> (Vec<Ty>, Ty) {
    let source = format!("class C\n  def m: {signature}\nend\n");
    let sigs = roundhouse::rbs::parse_signatures(&source)
        .unwrap_or_else(|e| panic!("`{signature}` should read: {e}"));
    let Ty::Fn { params, ret, .. } = &sigs.methods[0].1 else { panic!() };
    (params.iter().map(|p| (*p.ty).clone()).collect(), (**ret).clone())
}

fn class(id: &str) -> Ty {
    Ty::Class { id: ClassId(Symbol::new(id)), args: vec![].into() }
}

#[test]
fn singleton_is_the_class_it_names_even_for_a_builtin() {
    let (params, ret) = read("(singleton(Cart)) -> singleton(String)");
    assert_eq!(params[0], class("Cart"));
    assert_eq!(ret, class("String"), "the class String, not an instance of it");
    let (_, ret) = read("() -> Array[singleton(Cart)]");
    assert_eq!(ret, Ty::Array { elem: std::sync::Arc::new(class("Cart")) });
}

#[test]
fn bot_is_bottom_and_top_is_untyped() {
    let (params, ret) = read("(top) -> bot");
    assert_eq!(params[0], Ty::Untyped);
    assert_eq!(ret, Ty::Bottom);
}

#[test]
fn a_method_type_variable_is_untyped_not_a_class_called_t() {
    let (params, ret) = read("[T] (T) -> T?");
    assert_eq!(params[0], Ty::Untyped);
    assert_eq!(ret, Ty::Union { variants: vec![Ty::Untyped, Ty::Nil].into() });
}

#[test]
fn an_intersection_with_kernel_is_the_other_member() {
    let (params, _) = read("(Enumerable[Integer] & Kernel) -> void");
    assert_eq!(params[0], Ty::Class { id: ClassId(Symbol::new("Enumerable")), args: vec![Ty::Int].into() });
    let (params, _) = read("(Cart & Priced) -> void");
    assert_eq!(params[0], Ty::Untyped, "two real surfaces: claim neither");
}

#[test]
fn a_literal_type_is_a_value_of_its_class() {
    let (params, ret) = read("(:draft, \"x\", 1) -> true");
    assert_eq!(params, vec![Ty::Sym, Ty::Str, Ty::Int]);
    assert_eq!(ret, Ty::Bool);
}
