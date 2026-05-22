// Roundhouse Go v2 — recursive request-params value type.
//
// Hand-written, copied verbatim into the go2 overlay's `app/v2/`
// output. The Go analog of `runtime/rust/param_value.rs` (Rust
// `serde_json::Value` alias), `runtime/typescript/param_value.ts`
// (TS string | dict | array union), and `runtime/crystal/
// param_value.cr` (Crystal alias).
//
// Rails request params shape as a recursive tree of String, nested
// Hash, or Array. Go has no native sum type, so the realization is
// `any` (interface{}) — concrete runtime values hold `string`,
// `map[string]any`, or `[]any` per the recursive shape. The named
// alias preserves the RBS-declared `Roundhouse::ParamValue` →
// `RoundhouseParamValue` mapping so transpiled call sites
// (`params["id"]`, `params["comment"]["author"]`) line up without
// per-call-site coercion.

package v2

// RoundhouseParamValue — recursive param tree. Mirrors the RBS
// `Roundhouse::ParamValue` phantom class; concrete values are
// `string` (leaf), `map[string]RoundhouseParamValue` (nested
// hash), or `[]RoundhouseParamValue` (array). Aliased to `any`
// because Go interfaces can't express recursive sum types as a
// closed union; the alias gives the named type without ceremony.
type RoundhouseParamValue = any
