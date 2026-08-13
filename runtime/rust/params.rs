// Narrowing accessors over the recursive request-params tree.
//
// `Roundhouse::ParamValue` was a per-target TYPE WITH NO OPERATIONS, so
// every consumer narrowed at its own call site with `is_a?`, and each
// emitter recognized that as an IDIOM (a type test in `If`-condition
// position whose branches ARE the narrowed value) rather than modeling
// the test. Move the same fact to any other position and it falls off a
// cliff — measured, three spellings each broke a different target.
//
// So the operations live beside the type, hand-written per target the
// way `Db` is: these bodies inspect this target's representation, which
// is what makes them a primitive rather than framework logic.
//
// Semantics fixed by a Rails 8.1 oracle: `permit` keeps a blank "" and
// `Parameters#compact` drops only an explicit nil. So blank counts as
// provided; absent, JSON null, and non-scalar do not.

use crate::param_value::ParamValue;
use std::collections::HashMap;

pub struct Params;

impl Params {
    pub fn sub(params: HashMap<String, ParamValue>, key: &str) -> HashMap<String, ParamValue> {
        match params.get(key) {
            Some(serde_json::Value::Object(map)) => {
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
            _ => HashMap::new(),
        }
    }

    pub fn str(sub: HashMap<String, ParamValue>, key: &str, fallback: &str) -> String {
        match sub.get(key) {
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => fallback.to_string(),
        }
    }

    pub fn provided(sub: HashMap<String, ParamValue>, key: &str) -> bool {
        matches!(sub.get(key), Some(serde_json::Value::String(_)))
    }
}
