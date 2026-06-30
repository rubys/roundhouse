using System.Collections.Generic;

namespace Roundhouse;

// Turbo Streams broadcast sink — the object the model `after_*_commit`
// callbacks dispatch to (`Broadcasts.Append`/`Prepend`/`Replace`/`Remove`,
// each taking a kwargs bag lowered to a `Dictionary<string, object?>` carrying
// `stream`/`target`/`html`). Composes the `<turbo-stream>` wrapper and fans it
// out to /cable subscribers via `Cable`. Mirrors runtime/kotlin/broadcasts.kt.
public static class Broadcasts
{
    public static void Append(Dictionary<string, object?> opts) => Record("append", opts);
    public static void Prepend(Dictionary<string, object?> opts) => Record("prepend", opts);
    public static void Replace(Dictionary<string, object?> opts) => Record("replace", opts);
    public static void Remove(Dictionary<string, object?> opts) => Record("remove", opts);

    private static void Record(string action, Dictionary<string, object?> opts)
    {
        if (opts.GetValueOrDefault("stream") is not string stream) return;
        var target = opts.GetValueOrDefault("target") as string ?? "";
        var html = opts.GetValueOrDefault("html") as string ?? "";
        Cable.Dispatch(stream, Cable.TurboStreamHtml(action, target, html));
    }
}
