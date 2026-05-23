// Roundhouse go2 router glue.
//
// Phase 4 minimum wedge — provides `Router()` returning a net/http
// handler so the emitted `main.go` template compiles. Empty
// `http.ServeMux` for now; per-route binding (`mux.HandleFunc(verb,
// path, handler)`) lands in the next wedge that wires the transpiled
// `flatten_routes(app)` table into concrete handlers (rust2 analog:
// wedge 2c.2's `axum::Router::new().route(...)` chain).
//
// Kept separate from `server.go` so router-only changes don't churn
// the boot path; mirrors `runtime/rust/router.rs`-vs-`server.rs`
// separation.

package v2

import "net/http"

// Router returns the application's HTTP handler. Today: empty mux
// (every request 404s). The transpiled `app/v2/router.go` already
// holds the `ActionDispatchRouter_match` table; the next wedge
// reflects each entry into a `mux.HandleFunc` call against the
// matching controller-action wrapper.
func Router() http.Handler {
	return http.NewServeMux()
}
