//! M2: minimal VM executing hand-written bytecode.
//!
//! Covers arithmetic, locals, typed comparisons, conditional and
//! unconditional branches, stack manipulation, string literal loads,
//! and the opcodes deliberately deferred to later milestones (which
//! should fail cleanly with `NotYetSupported`, not panic).
//!
//! Tests hand-write `Program` values because no bytecode emitter
//! exists yet — that lands in M3 (post 6/6 target parity).

use roundhouse::bytecode::{Op, Program, StrId, Value, Vm, VmError};

/// Run a complete program from pc=0, returning the top of stack at
/// termination. Shared helper to keep per-test boilerplate minimal.
fn run(program: Program, locals_count: usize) -> Result<Option<Value>, VmError> {
    let mut vm = Vm::new(&program).with_locals(locals_count);
    vm.run()
}

fn program_with_code(code: Vec<Op>) -> Program {
    let mut p = Program::new();
    p.code = code;
    p
}

// ── Literal pushes + Return ──────────────────────────────────────

#[test]
fn load_i64_then_return() {
    let p = program_with_code(vec![Op::LoadI64 { value: 42 }, Op::Return]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(42)));
}

#[test]
fn load_bool_then_return() {
    let p = program_with_code(vec![Op::LoadBool { value: true }, Op::Return]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Bool(true)));
}

#[test]
fn load_nil_then_return() {
    let p = program_with_code(vec![Op::LoadNil, Op::Return]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Nil));
}

#[test]
fn load_f64_then_return() {
    let p = program_with_code(vec![Op::LoadF64 { value: 3.5 }, Op::Return]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Float(3.5)));
}

#[test]
fn load_str_from_pool() {
    let mut p = program_with_code(vec![Op::LoadStr { id: StrId(0) }, Op::Return]);
    p.string_pool = vec!["hello".into()];
    assert_eq!(run(p, 0).unwrap(), Some(Value::Str("hello".into())));
}

#[test]
fn load_str_with_invalid_id_errors() {
    let p = program_with_code(vec![Op::LoadStr { id: StrId(7) }, Op::Return]);
    assert!(matches!(run(p, 0), Err(VmError::InvalidStringId(7))));
}

// ── Arithmetic ────────────────────────────────────────────────────

#[test]
fn arithmetic_precedence_via_stack_order() {
    // (42 + 8) * 2 = 100
    let p = program_with_code(vec![
        Op::LoadI64 { value: 42 },
        Op::LoadI64 { value: 8 },
        Op::AddI64,
        Op::LoadI64 { value: 2 },
        Op::MulI64,
        Op::Return,
    ]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(100)));
}

#[test]
fn subtraction_respects_operand_order() {
    // 10 - 3 = 7 (not 3 - 10 = -7)
    let p = program_with_code(vec![
        Op::LoadI64 { value: 10 },
        Op::LoadI64 { value: 3 },
        Op::SubI64,
        Op::Return,
    ]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(7)));
}

#[test]
fn division_by_zero_errors() {
    let p = program_with_code(vec![
        Op::LoadI64 { value: 10 },
        Op::LoadI64 { value: 0 },
        Op::DivI64,
        Op::Return,
    ]);
    assert_eq!(run(p, 0), Err(VmError::DivisionByZero));
}

#[test]
fn integer_overflow_wraps() {
    // i64::MAX + 1 wraps to i64::MIN (we use wrapping_add)
    let p = program_with_code(vec![
        Op::LoadI64 { value: i64::MAX },
        Op::LoadI64 { value: 1 },
        Op::AddI64,
        Op::Return,
    ]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(i64::MIN)));
}

// ── Locals ────────────────────────────────────────────────────────

#[test]
fn local_store_load_cycle() {
    // local[0] = 10; (local[0] + 5) -> 15
    let p = program_with_code(vec![
        Op::LoadI64 { value: 10 },
        Op::StoreLocal { slot: 0 },
        Op::LoadLocal { slot: 0 },
        Op::LoadI64 { value: 5 },
        Op::AddI64,
        Op::Return,
    ]);
    assert_eq!(run(p, 1).unwrap(), Some(Value::Int(15)));
}

#[test]
fn local_starts_as_nil() {
    let p = program_with_code(vec![Op::LoadLocal { slot: 0 }, Op::Return]);
    assert_eq!(run(p, 1).unwrap(), Some(Value::Nil));
}

#[test]
fn load_local_out_of_range_errors() {
    let p = program_with_code(vec![Op::LoadLocal { slot: 5 }, Op::Return]);
    assert_eq!(run(p, 1), Err(VmError::InvalidLocalSlot(5)));
}

#[test]
fn store_local_out_of_range_errors() {
    let p = program_with_code(vec![
        Op::LoadI64 { value: 1 },
        Op::StoreLocal { slot: 5 },
        Op::Return,
    ]);
    assert_eq!(run(p, 1), Err(VmError::InvalidLocalSlot(5)));
}

// ── Comparisons ───────────────────────────────────────────────────

#[test]
fn all_typed_comparisons_produce_expected_bool() {
    let cases: &[(Op, i64, i64, bool)] = &[
        (Op::EqI64, 5, 5, true),
        (Op::EqI64, 5, 6, false),
        (Op::NeI64, 5, 5, false),
        (Op::NeI64, 5, 6, true),
        (Op::LtI64, 5, 6, true),
        (Op::LtI64, 6, 5, false),
        (Op::LeI64, 5, 5, true),
        (Op::LeI64, 6, 5, false),
        (Op::GtI64, 6, 5, true),
        (Op::GtI64, 5, 5, false),
        (Op::GeI64, 5, 5, true),
        (Op::GeI64, 5, 6, false),
    ];
    for (op, a, b, expected) in cases {
        let p = program_with_code(vec![
            Op::LoadI64 { value: *a },
            Op::LoadI64 { value: *b },
            op.clone(),
            Op::Return,
        ]);
        assert_eq!(
            run(p, 0).unwrap(),
            Some(Value::Bool(*expected)),
            "{:?} on {} {} should be {}",
            op,
            a,
            b,
            expected
        );
    }
}

// ── Control flow ──────────────────────────────────────────────────

#[test]
fn if_true_branch_selects_then() {
    // (5 > 3) ? 100 : 200 → 100
    //  pc      op
    //   0      LoadI64 5
    //   1      LoadI64 3
    //   2      GtI64            // pushes true
    //   3      JumpIfFalse +2   // (pc after=4, skip to 6 if false)
    //   4      LoadI64 100      // then branch
    //   5      Jump +1          // (pc after=6, skip else to 7)
    //   6      LoadI64 200      // else branch
    //   7      Return
    let p = program_with_code(vec![
        Op::LoadI64 { value: 5 },
        Op::LoadI64 { value: 3 },
        Op::GtI64,
        Op::JumpIfFalse { offset: 2 },
        Op::LoadI64 { value: 100 },
        Op::Jump { offset: 1 },
        Op::LoadI64 { value: 200 },
        Op::Return,
    ]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(100)));
}

#[test]
fn if_false_branch_selects_else() {
    // Same shape, but condition is false.
    let p = program_with_code(vec![
        Op::LoadI64 { value: 3 },
        Op::LoadI64 { value: 5 },
        Op::GtI64,
        Op::JumpIfFalse { offset: 2 },
        Op::LoadI64 { value: 100 },
        Op::Jump { offset: 1 },
        Op::LoadI64 { value: 200 },
        Op::Return,
    ]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(200)));
}

#[test]
fn backward_jump_iterates_until_condition() {
    // Countdown loop:
    //   local[0] = 3
    //   while local[0] > 0 { local[0] -= 1 }
    //   return local[0]
    //
    //   pc       op
    //    0       LoadI64 3
    //    1       StoreLocal 0          // local[0] = 3
    //    2       LoadLocal 0           // loop top: push local[0]
    //    3       LoadI64 0
    //    4       LeI64                 // push (local[0] <= 0)
    //    5       JumpIfTrue +5         // pc after=6, target=11 (exit)
    //    6       LoadLocal 0
    //    7       LoadI64 1
    //    8       SubI64
    //    9       StoreLocal 0          // local[0] -= 1
    //   10       Jump -9               // pc after=11, target=2 (loop top)
    //   11       LoadLocal 0           // exit: push final value
    //   12       Return
    let p = program_with_code(vec![
        Op::LoadI64 { value: 3 },
        Op::StoreLocal { slot: 0 },
        Op::LoadLocal { slot: 0 },
        Op::LoadI64 { value: 0 },
        Op::LeI64,
        Op::JumpIfTrue { offset: 5 },
        Op::LoadLocal { slot: 0 },
        Op::LoadI64 { value: 1 },
        Op::SubI64,
        Op::StoreLocal { slot: 0 },
        Op::Jump { offset: -9 },
        Op::LoadLocal { slot: 0 },
        Op::Return,
    ]);
    assert_eq!(run(p, 1).unwrap(), Some(Value::Int(0)));
}

#[test]
fn negative_pc_jump_errors() {
    // Jump with large negative offset runs off the front.
    let p = program_with_code(vec![Op::Jump { offset: -100 }]);
    assert!(matches!(run(p, 0), Err(VmError::PcOutOfBounds(_))));
}

// ── Stack manipulation ───────────────────────────────────────────

#[test]
fn dup_doubles_top_of_stack() {
    // 5 Dup AddI64 → 10
    let p = program_with_code(vec![
        Op::LoadI64 { value: 5 },
        Op::Dup,
        Op::AddI64,
        Op::Return,
    ]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(10)));
}

#[test]
fn pop_discards_top() {
    // Push 5, 7; Pop → top is 5
    let p = program_with_code(vec![
        Op::LoadI64 { value: 5 },
        Op::LoadI64 { value: 7 },
        Op::Pop,
        Op::Return,
    ]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(5)));
}

// ── Error conditions ─────────────────────────────────────────────

#[test]
fn stack_underflow_on_binary_op() {
    let p = program_with_code(vec![Op::AddI64]);
    assert_eq!(run(p, 0), Err(VmError::StackUnderflow));
}

#[test]
fn stack_underflow_on_pop() {
    let p = program_with_code(vec![Op::Pop]);
    assert_eq!(run(p, 0), Err(VmError::StackUnderflow));
}

#[test]
fn stack_underflow_on_dup() {
    let p = program_with_code(vec![Op::Dup]);
    assert_eq!(run(p, 0), Err(VmError::StackUnderflow));
}

#[test]
fn type_mismatch_bool_in_arithmetic() {
    let p = program_with_code(vec![
        Op::LoadBool { value: true },
        Op::LoadI64 { value: 1 },
        Op::AddI64,
        Op::Return,
    ]);
    assert!(matches!(
        run(p, 0),
        Err(VmError::TypeMismatch {
            expected: "Int",
            ..
        })
    ));
}

#[test]
fn type_mismatch_int_in_conditional() {
    let p = program_with_code(vec![
        Op::LoadI64 { value: 1 },
        Op::JumpIfFalse { offset: 0 },
    ]);
    assert!(matches!(
        run(p, 0),
        Err(VmError::TypeMismatch {
            expected: "Bool",
            ..
        })
    ));
}

// ── Opcodes deferred to later milestones ──────────────────────────

#[test]
fn call_user_with_invalid_fn_id_errors() {
    // CallUser is supported post-M3c, but an empty user_fns table
    // means fn_id 0 is out of range — the VM must surface that
    // cleanly rather than panicking on the index lookup.
    use roundhouse::bytecode::UserFnId;
    let p = program_with_code(vec![Op::CallUser {
        fn_id: UserFnId(0),
        argc: 0,
    }]);
    assert_eq!(run(p, 0), Err(VmError::InvalidUserFnId(0)));
}

#[test]
fn call_rt_not_yet_supported() {
    use roundhouse::bytecode::RtFnId;
    let p = program_with_code(vec![Op::CallRt {
        rt_id: RtFnId(0),
        argc: 0,
    }]);
    assert_eq!(run(p, 0), Err(VmError::NotYetSupported("call_rt")));
}

#[test]
fn concat_str_not_yet_supported() {
    let p = program_with_code(vec![Op::ConcatStr]);
    assert_eq!(run(p, 0), Err(VmError::NotYetSupported("concat_str")));
}

#[test]
fn collection_opcodes_not_yet_supported() {
    for (op, name) in [
        (Op::NewArray { len: 0 }, "new_array"),
        (Op::NewHash { entries: 0 }, "new_hash"),
        (Op::IndexLoad, "index_load"),
        (Op::IndexStore, "index_store"),
        (Op::InterpStr { parts: 0 }, "interp_str"),
    ] {
        let p = program_with_code(vec![op]);
        assert_eq!(run(p, 0), Err(VmError::NotYetSupported(name)));
    }
}

// ── User-defined function calls (M3c) ────────────────────────────

use roundhouse::bytecode::{RtFnId, UserFn, UserFnId};

fn program_with_main_and_user_fn(
    main_code: Vec<Op>,
    fn_name: &str,
    fn_arity: u8,
    fn_locals: u16,
    fn_body: Vec<Op>,
) -> Program {
    let mut p = Program::new();
    let code_offset = main_code.len() as u32;
    p.user_fns.push(UserFn {
        name: fn_name.into(),
        code_offset,
        arity: fn_arity,
        locals_count: fn_locals,
    });
    p.code = main_code;
    p.code.extend(fn_body);
    p
}

#[test]
fn call_user_with_zero_args_returns_value() {
    // main: CallUser 0 argc=0; Return
    // fn "answer": LoadI64 42; Return
    let main = vec![
        Op::CallUser {
            fn_id: UserFnId(0),
            argc: 0,
        },
        Op::Return,
    ];
    let body = vec![Op::LoadI64 { value: 42 }, Op::Return];
    let p = program_with_main_and_user_fn(main, "answer", 0, 0, body);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(42)));
}

#[test]
fn call_user_passes_args_into_local_slots() {
    // main: LoadI64 10; LoadI64 20; CallUser 0 argc=2; Return
    // fn "add": LoadLocal 0; LoadLocal 1; AddI64; Return
    let main = vec![
        Op::LoadI64 { value: 10 },
        Op::LoadI64 { value: 20 },
        Op::CallUser {
            fn_id: UserFnId(0),
            argc: 2,
        },
        Op::Return,
    ];
    let body = vec![
        Op::LoadLocal { slot: 0 },
        Op::LoadLocal { slot: 1 },
        Op::AddI64,
        Op::Return,
    ];
    let p = program_with_main_and_user_fn(main, "add", 2, 2, body);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(30)));
}

#[test]
fn nested_call_stacks_frames_correctly() {
    // main: CallUser outer (argc=0); Return
    // outer: LoadI64 7; CallUser inner (argc=1); Return     // expects: inner(7) on top of stack
    // inner: LoadLocal 0; LoadI64 1; AddI64; Return         // returns n+1
    //
    // Expected program result: 8.
    let mut p = Program::new();
    let outer_id = UserFnId(0);
    let inner_id = UserFnId(1);
    // We reserve both user_fns up-front; code_offset will be set below.
    p.user_fns.push(UserFn {
        name: "outer".into(),
        code_offset: 0,
        arity: 0,
        locals_count: 0,
    });
    p.user_fns.push(UserFn {
        name: "inner".into(),
        code_offset: 0,
        arity: 1,
        locals_count: 1,
    });

    // main
    p.code.push(Op::CallUser {
        fn_id: outer_id,
        argc: 0,
    });
    p.code.push(Op::Return);

    // outer body
    p.user_fns[0].code_offset = p.code.len() as u32;
    p.code.push(Op::LoadI64 { value: 7 });
    p.code.push(Op::CallUser {
        fn_id: inner_id,
        argc: 1,
    });
    p.code.push(Op::Return);

    // inner body
    p.user_fns[1].code_offset = p.code.len() as u32;
    p.code.push(Op::LoadLocal { slot: 0 });
    p.code.push(Op::LoadI64 { value: 1 });
    p.code.push(Op::AddI64);
    p.code.push(Op::Return);

    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(8)));
}

#[test]
fn call_user_arity_mismatch_errors() {
    // fn expects 1 arg, main passes 0.
    let main = vec![Op::CallUser {
        fn_id: UserFnId(0),
        argc: 0,
    }];
    let body = vec![Op::LoadLocal { slot: 0 }, Op::Return];
    let p = program_with_main_and_user_fn(main, "takes_one", 1, 1, body);
    assert_eq!(
        run(p, 0),
        Err(VmError::ArityMismatch {
            expected: 1,
            got: 0,
        })
    );
}

#[test]
fn call_rt_still_not_yet_supported() {
    // CallRt stays deferred; make sure we didn't accidentally enable
    // it while adding CallUser.
    let p = program_with_code(vec![Op::CallRt {
        rt_id: RtFnId(0),
        argc: 0,
    }]);
    assert_eq!(run(p, 0), Err(VmError::NotYetSupported("call_rt")));
}

#[test]
fn sequential_calls_to_same_function_each_get_fresh_locals() {
    // fn "double": LoadLocal 0; LoadLocal 0; AddI64; Return
    // main: push 3, call, store local 0 (in caller's frame); push 5, call;
    //       pop current; read local 0 (should be 6, not mutated by the 2nd call)
    //
    // Actually simpler: just verify second call's locals aren't polluted
    // by the first call's locals.
    // main:
    //   LoadI64 3; CallUser 0 (1); StoreLocal 0  -> caller's local[0] = 6
    //   LoadI64 5; CallUser 0 (1)                -> pushes 10
    //   LoadLocal 0; AddI64; Return              -> 6 + 10 = 16
    let main = vec![
        Op::LoadI64 { value: 3 },
        Op::CallUser {
            fn_id: UserFnId(0),
            argc: 1,
        },
        Op::StoreLocal { slot: 0 },
        Op::LoadI64 { value: 5 },
        Op::CallUser {
            fn_id: UserFnId(0),
            argc: 1,
        },
        Op::LoadLocal { slot: 0 },
        Op::AddI64,
        Op::Return,
    ];
    let body = vec![
        Op::LoadLocal { slot: 0 },
        Op::LoadLocal { slot: 0 },
        Op::AddI64,
        Op::Return,
    ];
    let p = program_with_main_and_user_fn(main, "double", 1, 1, body);
    assert_eq!(run(p, 1).unwrap(), Some(Value::Int(16)));
}

// ── End-of-code semantics ────────────────────────────────────────

#[test]
fn implicit_return_at_end_of_code() {
    // No explicit Return; VM stops when pc runs past the last
    // instruction and returns top-of-stack.
    let p = program_with_code(vec![Op::LoadI64 { value: 7 }]);
    assert_eq!(run(p, 0).unwrap(), Some(Value::Int(7)));
}

#[test]
fn empty_program_returns_none() {
    let p = Program::new();
    assert_eq!(run(p, 0).unwrap(), None);
}
