using System;
using System.Collections;
using System.Collections.Generic;

namespace Roundhouse;

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

// Minimal JSON encoder the emitter targets for `JSON.generate(x)`.
public static class JsonBuilder
{
    public static string EncodeValue(object? value) =>
        System.Text.Json.JsonSerializer.Serialize(value);
}
