// GENERATED-CODE reference — the shape `src/emit/swift` must produce for
// `app/models/article.rb`. Mirrors the lowered IR (dump_ir Article):
// per-column accessors, `tableName`/`schemaColumns`, and the `fromStmt`
// cursor reader that `ArticlesController.index` calls. Columns are read in
// schema order: id(0), body(1), created_at(2), title(3), updated_at(4).
//
// Models are `class` (reference semantics, mutation, inheritance — plan
// delta 7). Class methods land as `static` members; Swift statics ARE
// inherited (unlike Kotlin companions), one per-model-emit headache fewer.
//
// `comments` is the `has_many :comments` association — lazily loaded the
// first time the view touches `article.comments.count`. A class computed
// property's getter may assign the cache without `mutating`.
final class Article {
    var id: Int = 0
    var body: String = ""
    var createdAt: String = ""
    var title: String = ""
    var updatedAt: String = ""

    private var persisted: Bool = false
    func markPersisted() { persisted = true }

    private var commentsCache: [Comment]? = nil
    var comments: [Comment] {
        if let cached = commentsCache { return cached }
        let loaded = Comment.forArticle(id)
        commentsCache = loaded
        return loaded
    }

    static func tableName() -> String { "articles" }
    static func schemaColumns() -> [String] { ["id", "body", "created_at", "title", "updated_at"] }

    static func fromStmt(_ stmt: Int) -> Article {
        let instance = Article()
        instance.id = Db.columnInt(stmt, 0)
        instance.body = Db.columnText(stmt, 1)
        instance.createdAt = Db.columnText(stmt, 2)
        instance.title = Db.columnText(stmt, 3)
        instance.updatedAt = Db.columnText(stmt, 4)
        instance.markPersisted()
        return instance
    }
}
