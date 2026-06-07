package roundhouse

// GENERATED-CODE reference for `app/models/comment.rb` — minimal slice the
// index view needs (`article.comments.size`). Columns in schema order:
// id(0), article_id(1), body(2), commenter(3), created_at(4), updated_at(5).
//
// `forArticle` is the inverse of `has_many :comments`: a per-article
// SELECT, the lazy load that backs `Article.comments`. (The eager-load /
// includes path is out of Phase R scope.)
class Comment {
    var id: Long = 0
    var articleId: Long = 0
    var body: String = ""
    var commenter: String = ""
    var createdAt: String = ""
    var updatedAt: String = ""

    companion object {
        fun fromStmt(stmt: Long): Comment {
            val instance = Comment()
            instance.id = Db.columnInt(stmt, 0)
            instance.articleId = Db.columnInt(stmt, 1)
            instance.body = Db.columnText(stmt, 2)
            instance.commenter = Db.columnText(stmt, 3)
            instance.createdAt = Db.columnText(stmt, 4)
            instance.updatedAt = Db.columnText(stmt, 5)
            return instance
        }

        fun forArticle(articleId: Long): MutableList<Comment> {
            val stmt = Db.prepare(
                "SELECT id, article_id, body, commenter, created_at, updated_at " +
                    "FROM comments WHERE article_id = " + Db.escapeInt(articleId)
            )
            val results = ArrayList<Comment>()
            while (Db.step(stmt)) {
                results.add(fromStmt(stmt))
            }
            Db.finalize(stmt)
            return results
        }
    }
}
