using System;

namespace Roundhouse;

// `Time.now().utc.iso8601` is the sole Time API the framework runtime uses
// (`ActiveRecord::Base#fill_timestamps`). The emitter renders that chain as a
// method call then two property reads, so `Now()` returns a `TimeInstant`
// whose `Utc`/`Iso8601` are properties.
public static class Time
{
    public static TimeInstant Now() => new TimeInstant();
}

public class TimeInstant
{
    public TimeInstant Utc => this;
    public string Iso8601 => DateTime.UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ");
}
