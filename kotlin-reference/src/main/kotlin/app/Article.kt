package roundhouse

// GENERATED-CODE reference — the shape `src/emit/kotlin` must produce for
// `app/models/article.rb`. Mirrors the lowered IR (dump_ir Article):
// per-column accessors, `tableName`/`schemaColumns`, and the `fromStmt`
// cursor reader that `ArticlesController.index` calls. Columns are read in
// schema order: id(0), body(1), created_at(2), title(3), updated_at(4).
//
// `comments` is the `has_many :comments` association — lazily loaded the
// first time the view touches `article.comments.size`.
class Article {
    var id: Long = 0
    var body: String = ""
    var createdAt: String = ""
    var title: String = ""
    var updatedAt: String = ""

    private var persisted: Boolean = false
    fun markPersisted() { persisted = true }

    private var commentsCache: MutableList<Comment>? = null
    val comments: MutableList<Comment>
        get() = commentsCache ?: Comment.forArticle(id).also { commentsCache = it }

    companion object {
        fun tableName(): String = "articles"
        fun schemaColumns(): List<String> = listOf("id", "body", "created_at", "title", "updated_at")

        fun fromStmt(stmt: Long): Article {
            val instance = Article()
            instance.id = Db.columnInt(stmt, 0)
            instance.body = Db.columnText(stmt, 1)
            instance.createdAt = Db.columnText(stmt, 2)
            instance.title = Db.columnText(stmt, 3)
            instance.updatedAt = Db.columnText(stmt, 4)
            instance.markPersisted()
            return instance
        }
    }
}
