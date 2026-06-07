package roundhouse

// Entry point. `BLOG_DB` / `PORT` mirror the env the other targets read in
// scripts/compare + scripts/bench so a future Kotlin cell drops in.
fun main() {
    val dbPath = System.getenv("BLOG_DB") ?: "storage/development.sqlite3"
    val port = (System.getenv("PORT") ?: "9000").toInt()
    Server.start(dbPath, port)
}
