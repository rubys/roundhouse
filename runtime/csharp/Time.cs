using System;

namespace Roundhouse;

// `Time.now().utc.iso8601` is the sole Time API the framework runtime uses
// (`ActiveRecord::Base#fill_timestamps`). The emitter renders that chain as a
// method call then two property reads, so `now()` returns a `TimeInstant`
// whose `utc`/`iso8601` are properties.
public static class Time
{
    public static TimeInstant now() => new TimeInstant();
}

public class TimeInstant
{
    public TimeInstant utc => this;
    public string iso8601 => DateTime.UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ");
}
