package roundhouse

// The recursive params value type — the Kotlin analog of
// `runtime/crystal/param_value.cr` (`String | Hash | Array`) and
// `runtime/typescript/param_value.ts`. Ruby's untyped nested params Hash
// lowers to this closed union; `<Resource>Params.from_raw` narrows via
// `is Str` / `is Dict` at access sites.
//
// Unused by GET /articles, but included in Phase R to lock the shape the
// emitter targets for the params layer.
sealed interface ParamValue {
    @JvmInline value class Str(val value: String) : ParamValue
    @JvmInline value class Dict(val value: MutableMap<String, ParamValue>) : ParamValue
    @JvmInline value class Arr(val value: MutableList<ParamValue>) : ParamValue
}
