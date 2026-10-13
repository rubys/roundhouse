//! Recursive Rails-params value type — the rust analog of
//! `runtime/typescript/param_value.ts` and `runtime/crystal/param_value.cr`.
//!
//! Rails request params include String, numeric, boolean, and null
//! leaves plus recursive Vec<ParamValue> and HashMap<String,
//! ParamValue>. The TS and Crystal siblings carry the same JSON value
//! members in their respective recursive unions.
//!
//! In rust2 Phase 3 this is a re-export alias to `serde_json::Value`
//! — same recursive shape, already familiar to every emit path that
//! lowers `untyped`. Concrete enum can replace this later if the
//! typed-value discipline gets tighter (Ty::Untyped reform).

pub type ParamValue = serde_json::Value;
