import Foundation

enum I18nHost {
    // Not one static var: a request's dispatch runs on one NIOThreadPool thread, several at once
    private static let locale = RhThreadLocal<String>({ "" })
    private static let tableLock = NSLock()
    private static var table: String? = nil

    static func resetRequest() {
        locale.value = ""
    }

    static func localeName() -> String {
        locale.value
    }

    @discardableResult
    static func setLocaleName(_ name: String) -> String {
        locale.value = name
        return name
    }

    static func envName() -> String {
        let env = ProcessInfo.processInfo.environment
        return env["RAILS_ENV"] ?? env["RACK_ENV"] ?? "development"
    }

    // Not the working directory alone: a test runner may start below the app root
    static func tableText() -> String {
        tableLock.lock()
        defer { tableLock.unlock() }
        if let table { return table }
        var dir = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        while true {
            let file = dir.appendingPathComponent("config/i18n_translations")
            if let text = try? String(contentsOf: file, encoding: .utf8) {
                table = text
                return text
            }
            let parent = dir.deletingLastPathComponent()
            if parent.path == dir.path { break }
            dir = parent
        }
        fatalError("config/i18n_translations is in no directory above " + FileManager.default.currentDirectoryPath)
    }
}
