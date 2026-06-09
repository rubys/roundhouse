// The recursive params value type — the Swift analog of
// `runtime/crystal/param_value.cr` (`String | Hash | Array`) and
// `kotlin-reference/runtime/ParamValue.kt` (a sealed interface). Ruby's
// untyped nested params Hash lowers to this closed union; an enum with
// associated values is the idiomatic Swift spelling, and
// `<Resource>Params.from_raw` narrows via `case .str` / `case .dict`
// pattern matches at access sites.
//
// Unused by GET /articles, but included in Phase R to lock the shape the
// emitter targets for the params layer.
enum ParamValue {
    case str(String)
    case dict([String: ParamValue])
    case arr([ParamValue])
}
