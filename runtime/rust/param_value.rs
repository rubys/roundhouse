//! Recursive Rails-params value type — the rust analog of
//! `runtime/typescript/param_value.ts` and `runtime/crystal/param_value.cr`.
//!
//! Rails request params shape as a recursive tree of String,
//! Vec<ParamValue>, or HashMap<String, ParamValue>. The TS sibling
//! declares it as a union of (string | string[] | { [k: string]:
//! ParamValue }); Crystal as `alias ParamValue = String | Hash(String,
//! ParamValue) | Array(ParamValue)`.
//!
//! In rust2 Phase 3 this is a re-export alias to `serde_json::Value`
//! — same recursive shape, already familiar to every emit path that
//! lowers `untyped`. Concrete enum can replace this later if the
//! typed-value discipline gets tighter (Ty::Untyped reform).

pub type ParamValue = serde_json::Value;
