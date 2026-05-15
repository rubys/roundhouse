# Type-seed pins for spinel-AOT static inference. Spinel propagates
# parameter types one-way (caller → callee); methods exported by the
# framework runtime or emitted model classes whose typed callers
# aren't reachable from a given test file's call graph get their
# params defaulted to `int`, then their body operations (`row["id"]`,
# `table.length`) fail to dispatch and emit "(emitting 0)" no-ops.
#
# The methods below are exercised in production paths (Base#_adapter_reload
# → `assign_from_row(row)`, controller actions → `*Row.from_raw(params)`,
# `dispatch_request` → `Router.match(method, path, table)`) but those
# call sites aren't reached from many test compilations. The typed
# calls below give spinel one typed call site per method at
# module-load time, pinning param types globally.
#
# Loaded by test_helper.rb after the framework runtime require chain
# is complete. Idempotent — `require_relative` ensures each model
# file loads at most once even though individual test files also
# require them.
#
# Hand-written sibling of the type-seeding pattern in `runtime/tep/tep.rb`
# (which pins `Tep::Response#set_cookie`/`#start_stream` for the same
# reason — those are framework methods that the spinel-reachable code
# in the CGI path doesn't directly exercise).

require_relative "../app/models/article"
require_relative "../app/models/article_row"
require_relative "../app/models/comment"
require_relative "../app/models/comment_row"

# Hash with the union of columns Article/Comment care about. String
# keys (matches the production sqlite row shape). Values are typed
# so spinel can read off param types from the call sites below.
_seed_row = {
  "id" => 0,
  "body" => "",
  "title" => "",
  "created_at" => "",
  "updated_at" => "",
  "article_id" => 0,
}

# Params-struct factories — pin `from_raw(row)`'s param type.
ArticleRow.from_raw(_seed_row)
CommentRow.from_raw(_seed_row)

# Row-assignment overrides — pin `assign_from_row(row)`'s param type
# on each concrete subclass (Base's abstract stub raises; the typed
# call here goes through the override that actually indexes `row`).
Article.new.assign_from_row(_seed_row)
Comment.new.assign_from_row(_seed_row)

# Router.match's table param — pin to the Routes.table shape. The
# match-on-"/_type_seed" returns nil at runtime (no route matches);
# harmless side effect.
ActionDispatch::Router.match("GET", "/_type_seed", Routes.table)
