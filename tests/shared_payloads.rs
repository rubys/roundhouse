use roundhouse::ident::Symbol;
use roundhouse::ty::{Param, ParamKind, Row, Ty};
use std::sync::Arc;

#[test]
fn sharing_preserves_the_existing_wire_shape() {
    let value = Ty::Array {
        elem: Arc::new(Ty::Tuple {
            elems: vec![Ty::Str, Ty::Int].into(),
        }),
    };
    let expected =
        r#"{"kind":"array","elem":{"kind":"tuple","elems":[{"kind":"str"},{"kind":"int"}]}}"#;
    assert_eq!(serde_json::to_string(&value).unwrap(), expected);
    assert_eq!(serde_json::from_str::<Ty>(expected).unwrap(), value);
}

#[test]
fn vector_mutation_and_consumption_leave_other_owners_unchanged() {
    let original: roundhouse::shared::Shared<Vec<Ty>> = vec![Ty::Str, Ty::Int].into();
    let mut edited = original.clone();
    assert!(original.ptr_eq(&edited));
    edited[0] = Ty::Bool;
    assert!(!original.ptr_eq(&edited));
    assert_eq!(
        edited.into_iter().collect::<Vec<_>>(),
        vec![Ty::Bool, Ty::Int]
    );
    assert_eq!(&*original, &vec![Ty::Str, Ty::Int]);
}

#[test]
fn record_equality_and_insertion_order_keep_their_existing_contract() {
    let mut a = Row {
        fields: [(Symbol::from("a"), Ty::Int), (Symbol::from("b"), Ty::Str)]
            .into_iter()
            .collect(),
        rest: None,
    };
    let retained = a.clone();
    let b = Row {
        fields: [(Symbol::from("b"), Ty::Str), (Symbol::from("a"), Ty::Int)]
            .into_iter()
            .collect(),
        rest: None,
    };
    assert_eq!(a, b);
    assert_ne!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
    a.fields.insert(Symbol::from("a"), Ty::Bool);
    assert_eq!(retained, b);
    assert_ne!(a, retained);
}

#[test]
fn parameter_mutation_detaches_both_the_vector_and_type_edge() {
    let retained: roundhouse::shared::Shared<Vec<Param>> = vec![Param {
        name: Symbol::from("value"),
        ty: Arc::new(Ty::Str),
        kind: ParamKind::Required,
    }]
    .into();
    let mut edited = retained.clone();
    *Arc::make_mut(&mut edited[0].ty) = Ty::Int;
    assert_eq!(*retained[0].ty, Ty::Str);
    assert_eq!(*edited[0].ty, Ty::Int);
}
