// Narrowing accessors over the recursive request-params tree.
//
// `Roundhouse::ParamValue` was a per-target TYPE WITH NO OPERATIONS, so
// every consumer narrowed at its own call site with `is_a?`, and each
// emitter recognized that as an IDIOM rather than modeling the test.
// The operations live beside the type, hand-written per target the way
// `Db` is: these bodies inspect this target's representation, which is
// what makes them a primitive rather than framework logic.
//
// Semantics fixed by a Rails 8.1 oracle: `permit` keeps a blank "" and
// `Parameters#compact` drops only an explicit nil. So blank counts as
// provided; absent, JSON null, and non-scalar do not.

using System.Collections.Generic;

namespace Roundhouse;

public static class Params
{
    public static Dictionary<string, object?> Sub(Dictionary<string, object?> parameters, string key)
    {
        if (parameters.TryGetValue(key, out var value) && value is Dictionary<string, object?> nested)
        {
            return nested;
        }
        return new Dictionary<string, object?>();
    }

    public static string Str(Dictionary<string, object?> sub, string key, string fallback)
    {
        if (sub.TryGetValue(key, out var value) && value is string s)
        {
            return s;
        }
        return fallback;
    }

    public static bool Provided(Dictionary<string, object?> sub, string key)
    {
        return sub.TryGetValue(key, out var value) && value is string;
    }

    // The scalar readers the params lowering emits, and its record of
    // multiparameter keys. Those keys (`title(1)`) are modeled on the
    // Ruby family only (runtime/ruby/params.rb); this target ignores
    // them, as it always has, so these pass through.
    public static bool ScalarProvided(Dictionary<string, object?> sub, string key) => Provided(sub, key);

    public static string Scalar(Dictionary<string, object?> sub, string key) => Str(sub, key, "");

    public static List<string> NoFields() => new List<string>();

    public static List<string> MultiparameterFields(Dictionary<string, object?> sub, List<string> keys) => new List<string>();

    public static bool MultiparameterUnsupported(Dictionary<string, object?> sub, List<string> keys) => false;

    public static bool AnyOf(List<string> list, List<string> names) => names.Exists(list.Contains);
}
