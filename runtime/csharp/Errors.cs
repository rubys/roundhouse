using System;

namespace Roundhouse;

// AR exception classes the lowered finders raise — `Article.find` throws
// `RecordNotFound`, `Article.create!` throws `RecordInvalid`.
public class RecordNotFound : Exception
{
    public RecordNotFound(string message) : base(message) { }
}

public class RecordInvalid : Exception
{
    public object? Record { get; }
    public RecordInvalid(object? record) : base("Validation failed") => Record = record;
}
