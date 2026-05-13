//! HashMap helpers bridging Ruby's `Hash#merge` semantics.
//!
//! Ruby `hash.merge(other)` returns a new hash with `other`'s entries
//! layered on top. The transpiled framework runtime emits this on
//! HashMap-typed receivers with mixed K/V types — typically a literal
//! built from `(&str, &str)` or `(&str, String)` pairs merged with a
//! parameter-typed `HashMap<String, serde_json::Value>`. Generic Rust
//! merge traits can't bridge that K/V variance.
//!
//! `merge_attrs` is the pragmatic landing zone: it accepts any pair-
//! iterator on both sides whose K is `Into<String>` and V is
//! `Into<serde_json::Value>`, and produces a unified
//! `HashMap<String, serde_json::Value>`. That matches the
//! transpiled-runtime usage where the merged map is consumed by
//! `render_attrs`, `r#where`, etc. — call sites that don't need to
//! preserve the literal's narrower K/V types.

use serde_json::Value;
use std::collections::HashMap;

pub fn merge_attrs<I1, K1, V1, I2, K2, V2>(base: I1, other: I2) -> HashMap<String, Value>
where
    I1: IntoIterator<Item = (K1, V1)>,
    K1: Into<String>,
    V1: Into<Value>,
    I2: IntoIterator<Item = (K2, V2)>,
    K2: Into<String>,
    V2: Into<Value>,
{
    let mut out: HashMap<String, Value> = HashMap::new();
    for (k, v) in base {
        out.insert(k.into(), v.into());
    }
    for (k, v) in other {
        out.insert(k.into(), v.into());
    }
    out
}
