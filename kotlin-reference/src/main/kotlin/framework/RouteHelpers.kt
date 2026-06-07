package roundhouse

// URL helpers the views reference (`new_article_path`, `article_path`,
// `edit_article_path`). Normally generated from the routes table; the
// lowered IR calls them as `RouteHelpers.<name>`. In Ruby these are
// parameterless properties / path methods; here they are functions.
object RouteHelpers {
    fun newArticlePath(): String = "/articles/new"
    fun articlePath(id: Long): String = "/articles/$id"
    fun editArticlePath(id: Long): String = "/articles/$id/edit"
}
