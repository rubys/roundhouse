//! M1: bytecode format roundtrip.
//!
//! The first milestone for the bytecode target is "the format is
//! well-defined" — encode a `Program`, decode it, assert bit-identical.
//! Execution (the VM) and generation (the emitter) come later; this
//! test only exercises the types themselves.
//!
//! serde_json is the serializer here for legibility during development.
//! The format is deliberately not tied to JSON — any serde-compatible
//! format would pass these tests. A later milestone swaps in a binary
//! format (bincode or hand-rolled) once the VM consumes these at runtime.

use roundhouse::bytecode::{
    FORMAT_VERSION, Op, Program, RtFnId, StrId, SymId, UserFn, UserFnId,
};

fn roundtrip(p: &Program) -> Program {
    let json = serde_json::to_string(p).expect("serialize Program");
    serde_json::from_str(&json).expect("deserialize Program")
}

#[test]
fn empty_program_roundtrips() {
    let p = Program::new();
    assert_eq!(p, roundtrip(&p));
    assert_eq!(p.format_version, FORMAT_VERSION);
}

#[test]
fn constant_pools_preserve_order_and_content() {
    let mut p = Program::new();
    p.string_pool = vec!["hello".into(), "".into(), "with \"quote\"".into()];
    p.symbol_pool = vec!["title".into(), "body".into()];
    p.runtime_fns = vec!["save_model".into(), "find_by_id".into()];

    let out = roundtrip(&p);
    assert_eq!(out.string_pool, p.string_pool);
    assert_eq!(out.symbol_pool, p.symbol_pool);
    assert_eq!(out.runtime_fns, p.runtime_fns);
}

#[test]
fn user_fns_roundtrip_with_offsets_and_arity() {
    let mut p = Program::new();
    p.user_fns = vec![
        UserFn {
            name: "articles_index".into(),
            code_offset: 0,
            arity: 0,
            locals_count: 2,
        },
        UserFn {
            name: "articles_show".into(),
            code_offset: 42,
            arity: 1,
            locals_count: 3,
        },
    ];
    assert_eq!(p, roundtrip(&p));
}

#[test]
fn every_opcode_variant_roundtrips() {
    // Construct a program whose code section exercises every Op
    // variant. If any variant's serde tag is wrong or a field is
    // misnamed, deserialization fails here.
    let code = vec![
        // Literals
        Op::LoadI64 { value: 42 },
        Op::LoadI64 { value: -1 },
        Op::LoadI64 { value: i64::MAX },
        Op::LoadI64 { value: i64::MIN },
        Op::LoadF64 { value: 3.14 },
        Op::LoadF64 { value: 0.0 },
        Op::LoadStr { id: StrId(0) },
        Op::LoadSym { id: SymId(1) },
        Op::LoadBool { value: true },
        Op::LoadBool { value: false },
        Op::LoadNil,
        // Locals
        Op::LoadLocal { slot: 0 },
        Op::StoreLocal { slot: 5 },
        // Integer arithmetic
        Op::AddI64,
        Op::SubI64,
        Op::MulI64,
        Op::DivI64,
        // String concat
        Op::ConcatStr,
        // Comparisons
        Op::EqI64,
        Op::NeI64,
        Op::LtI64,
        Op::LeI64,
        Op::GtI64,
        Op::GeI64,
        // Control flow
        Op::Jump { offset: 12 },
        Op::Jump { offset: -4 },
        Op::JumpIfFalse { offset: 7 },
        Op::JumpIfTrue { offset: 3 },
        // Calls
        Op::CallUser {
            fn_id: UserFnId(2),
            argc: 3,
        },
        Op::CallRt {
            rt_id: RtFnId(9),
            argc: 0,
        },
        Op::Return,
        // Stack
        Op::Pop,
        Op::Dup,
        // Collections
        Op::NewArray { len: 4 },
        Op::NewHash { entries: 2 },
        Op::IndexLoad,
        Op::IndexStore,
        // Interpolation
        Op::InterpStr { parts: 3 },
    ];

    let mut p = Program::new();
    p.string_pool = vec!["hi".into()];
    p.symbol_pool = vec!["wat".into(), "title".into()];
    p.code = code;

    let out = roundtrip(&p);
    assert_eq!(out.code, p.code);
}

#[test]
fn whole_program_roundtrips_identically() {
    let mut p = Program::new();
    p.string_pool = vec!["greet".into(), "Hello, ".into()];
    p.symbol_pool = vec!["name".into()];
    p.runtime_fns = vec!["write_string".into()];
    p.user_fns = vec![UserFn {
        name: "greet".into(),
        code_offset: 0,
        arity: 1,
        locals_count: 0,
    }];
    p.code = vec![
        Op::LoadStr { id: StrId(1) },
        Op::LoadLocal { slot: 0 },
        Op::ConcatStr,
        Op::CallRt {
            rt_id: RtFnId(0),
            argc: 1,
        },
        Op::Return,
    ];

    assert_eq!(p, roundtrip(&p));
}
