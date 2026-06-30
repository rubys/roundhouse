using System.Collections.Generic;

namespace Roundhouse;

// The 9-method adapter contract `ActiveRecord::Base`'s class-level CRUD
// dispatches through (`ActiveRecord.adapter.find` / `.all` / `.where` / …).
// A phantom RBS-only class upstream — each target ships its own concrete
// implementation; here it's the compile-time contract.
//
// Like the Kotlin target, the functional adapter path is DROPPED for C#:
// `ActiveRecord.adapter` is never assigned. Real-blog's CRUD goes Db-direct
// through the per-model `_adapter_*` overrides (each model re-emits them
// calling `Db` itself), so Base's adapter-routing defaults (`where`/`find_by`)
// are dead for real-blog and would NRE if hit — the correct "unsupported"
// behavior. This abstract contract exists purely so those defaults type-check.
public abstract class AdapterInterface
{
    public abstract List<Dictionary<string, object?>> all(string tableName);
    public abstract Dictionary<string, object?>? find(string tableName, long id);
    public abstract List<Dictionary<string, object?>> where(string tableName, Dictionary<string, object?> conditions);
    public abstract long count(string tableName);
    public abstract bool existsPred(string tableName, long id);
    public abstract long insert(string tableName, Dictionary<string, object?> attributes);
    public abstract void update(string tableName, long id, Dictionary<string, object?> attributes);
    public abstract void delete(string tableName, long id);
    public abstract void truncate(string tableName);
}

// The default `ActiveRecord.adapter` value: throws on every call. The
// functional adapter path is dead for C# (models go Db-direct), so this exists
// only so the slot is non-null — if a Base default that routes through the
// adapter (`where`/`find_by`) is ever actually called, it fails loudly.
public sealed class NullAdapter : AdapterInterface
{
    private static System.NotSupportedException Unwired() =>
        new("ActiveRecord.adapter is not wired for the C# target (models are Db-direct)");

    public override List<Dictionary<string, object?>> all(string tableName) => throw Unwired();
    public override Dictionary<string, object?>? find(string tableName, long id) => throw Unwired();
    public override List<Dictionary<string, object?>> where(string tableName, Dictionary<string, object?> conditions) => throw Unwired();
    public override long count(string tableName) => throw Unwired();
    public override bool existsPred(string tableName, long id) => throw Unwired();
    public override long insert(string tableName, Dictionary<string, object?> attributes) => throw Unwired();
    public override void update(string tableName, long id, Dictionary<string, object?> attributes) => throw Unwired();
    public override void delete(string tableName, long id) => throw Unwired();
    public override void truncate(string tableName) => throw Unwired();
}
