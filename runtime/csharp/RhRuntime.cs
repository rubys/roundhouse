using System;
using System.Collections;
using System.Collections.Generic;

namespace Roundhouse;

// Module-level constants from the transpiled framework runtime (json_builder's
// ESCAPES, router's STATUS_CODES, …) have no top-level form in C#, so the
// emitter appends them as fragments of this partial class. This empty base
// declaration guarantees the type always exists, so the `using static
// Roundhouse.RuntimeConstants` in each runtime file resolves even when a file
// (or the whole runtime) carries no constants.
public static partial class RuntimeConstants { }

// Small bridging helpers the emitter targets for a few Ruby idioms that have
// no direct C# operator: `Hash#merge` and the `rescue` modifier.
public static class RhRuntime
{
    // `a.merge(b)` → a new dictionary, b winning on key collisions.
    public static Dictionary<string, object?> Merge(object? a, object? b)
    {
        var result = new Dictionary<string, object?>();
        Absorb(result, a);
        Absorb(result, b);
        return result;
    }

    private static void Absorb(Dictionary<string, object?> into, object? src)
    {
        if (src is IDictionary dict)
        {
            foreach (DictionaryEntry e in dict)
            {
                into[Convert.ToString(e.Key) ?? ""] = e.Value;
            }
        }
    }

    // `expr rescue fallback` — evaluate `body`, falling back on any exception.
    public static T Rescue<T>(Func<T> body, Func<T> fallback)
    {
        try { return body(); }
        catch { return fallback(); }
    }
}
