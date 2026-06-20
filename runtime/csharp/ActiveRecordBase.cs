using System;
using System.Collections.Generic;
using System.Linq;

namespace Roundhouse;

// The ActiveRecord::Base analog the emitted models extend and override. The
// `virtual` member names here must stay in sync with
// `src/emit/csharp.rs::register_runtime_base` (which tells the emitter which
// members take `override`).
//
// Phase 2 is compile-only: the `_adapter*` hooks each model overrides do the
// real SQL (via `Db`), but `Db` itself is a stub until Phase 3 wires the
// ADO.NET adapter — so this base compiles and runs as a no-op skeleton.
public class ActiveRecordBase
{
    public virtual long id { get; set; } = 0L;

    // The model `validate()` appends via `errors << msg` → `errors().Add(msg)`,
    // so `errors()` exposes the plain list.
    private readonly List<string> _errors = new();
    private bool _persisted = false;

    // ---- overridable framework hooks (per-model `override`s) ----
    public virtual void assignFromRow(Dictionary<string, object?> row) { }
    public virtual Dictionary<string, object?> attributes() => new();
    public virtual object? this[string name] { get => null; set { } }
    public virtual void fillTimestamps(bool creating) { }
    public virtual void validate() { }
    public virtual long _adapterInsert() => 0L;
    public virtual void _adapterUpdate() { }
    public virtual void _adapterDelete() { }
    public virtual ActiveRecordBase _adapterReload() => this;
    public virtual void beforeDestroy() { }
    public virtual string domPrefix() => "record";
    public virtual void afterCreateCommit() { }
    public virtual void afterUpdateCommit() { }
    public virtual void afterDestroyCommit() { }
    public virtual List<string> schemaColumns() =>
        throw new NotImplementedException("ActiveRecord::Base.schema_columns must be overridden");

    // ---- shared instance behavior (called, not overridden) ----
    public List<string> errors() => _errors;
    public bool persisted() => _persisted;
    public bool newRecord() => !_persisted;
    public void markPersistedBang() => _persisted = true;
    public string domId() => $"{domPrefix()}_{id}";

    public virtual bool save()
    {
        _errors.Clear();
        validate();
        if (_errors.Count > 0) return false;
        bool creating = !_persisted;
        fillTimestamps(creating);
        if (creating)
        {
            id = _adapterInsert();
            _persisted = true;
            afterCreateCommit();
        }
        else
        {
            _adapterUpdate();
            afterUpdateCommit();
        }
        return true;
    }

    public virtual bool destroy()
    {
        beforeDestroy();
        _adapterDelete();
        _persisted = false;
        afterDestroyCommit();
        return true;
    }

    public ActiveRecordBase reload() => _adapterReload();
}
