import Foundation

// Entry point. `BLOG_DB` / `PORT` mirror the env the other targets read in
// scripts/compare + scripts/bench so a future Swift cell drops in.
let dbPath = ProcessInfo.processInfo.environment["BLOG_DB"] ?? "storage/development.sqlite3"
let port = Int(ProcessInfo.processInfo.environment["PORT"] ?? "9000") ?? 9000
try await Server.start(dbPath: dbPath, port: port)
