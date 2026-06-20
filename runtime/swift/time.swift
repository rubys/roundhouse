import Foundation

enum Time {
    static func now() -> TimeInstant { TimeInstant(Date()) }
}

struct TimeInstant {
    let date: Date

    init(_ date: Date) { self.date = date }

    var utc: TimeInstant { self }

    var iso8601: String {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        let c = cal.dateComponents([.year, .month, .day, .hour, .minute, .second], from: date)
        return String(
            format: "%04d-%02d-%02dT%02d:%02d:%02dZ",
            c.year!, c.month!, c.day!, c.hour!, c.minute!, c.second!
        )
    }
}
