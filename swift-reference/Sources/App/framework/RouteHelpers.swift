// URL helpers the views reference (`new_article_path`, `article_path`,
// `edit_article_path`). Normally generated from the routes table; the
// lowered IR calls them as `RouteHelpers.<name>`. In Ruby these are
// parameterless properties / path methods; here they are static functions.
enum RouteHelpers {
    static func newArticlePath() -> String { "/articles/new" }
    static func articlePath(_ id: Int) -> String { "/articles/\(id)" }
    static func editArticlePath(_ id: Int) -> String { "/articles/\(id)/edit" }
}
