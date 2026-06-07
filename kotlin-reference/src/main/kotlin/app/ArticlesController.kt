package roundhouse

// GENERATED-CODE reference for `ArticlesController` — the `index` action
// only, transcribed from the lowered IR (dump_ir ArticlesController#index):
//
//   stmt = Db.prepare("SELECT ... FROM articles" + " ORDER BY created_at DESC")
//   while Db.step?(stmt): results << Article.from_stmt(stmt)
//   Db.finalize(stmt); @articles = results
//   render Views::Articles.index(@articles, @flash[:notice], @flash[:alert])
//
// `render(...)` lowers to assigning the controller's `body`. Flash slots
// are plain nullable fields here (Phase R has no flash plumbing).
class ArticlesController {
    var articles: MutableList<Article> = ArrayList()
    var requestFormat: String = "html"
    var flashNotice: String? = null
    var flashAlert: String? = null
    var body: String = ""
    var contentType: String = "text/html; charset=utf-8"

    fun index() {
        val stmt = Db.prepare(
            "SELECT id, body, created_at, title, updated_at FROM articles" + " ORDER BY created_at DESC"
        )
        val results = ArrayList<Article>()
        while (Db.step(stmt)) {
            results.add(Article.fromStmt(stmt))
        }
        Db.finalize(stmt)
        articles = results

        if (requestFormat == "json") {
            // JSON view out of Phase R scope; HTML branch is the gate.
            body = "[]"
            contentType = "application/json"
        } else {
            body = ArticlesView.index(articles, flashNotice, flashAlert)
        }
    }
}
