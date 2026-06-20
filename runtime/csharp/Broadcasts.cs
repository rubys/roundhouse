using System.Collections.Generic;

namespace Roundhouse;

// Turbo Streams broadcast sink — the object the model `after_*_commit`
// callbacks dispatch to (`Broadcasts.prepend`/`replace`/`remove`/`append`,
// each taking a kwargs bag lowered to a `Dictionary<string, object?>` carrying
// `stream`/`target`/`html`).
//
// **Phase 2 stub** — no Action Cable transport yet (Phase 4); the callbacks
// compile and no-op so the model layer builds.
public static class Broadcasts
{
    public static void append(Dictionary<string, object?> opts) { }
    public static void prepend(Dictionary<string, object?> opts) { }
    public static void replace(Dictionary<string, object?> opts) { }
    public static void remove(Dictionary<string, object?> opts) { }
}
