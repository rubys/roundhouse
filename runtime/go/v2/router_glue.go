// Roundhouse go2 router glue.
//
// Phase 4 wedge 15 — real dispatch. On every request:
//   1. ActionDispatchRouter_match (transpiled from
//      runtime/ruby/action_dispatch/router.rb) scans the
//      app-emitted RoutesTable for a verb+path match
//   2. Dispatch (emitted at app/v2/dispatch.go from the
//      controller list) constructs the controller, runs the
//      action, and returns the captured response state
//   3. We translate (body, status, contentType, location) into
//      a net/http response.
//
// Kept separate from server.go so router-only changes don't churn
// the boot path; mirrors `runtime/rust/router.rs`-vs-`server.rs`
// separation.

package v2

import (
	"net/http"
	"strings"
)

// Router returns the application's HTTP handler. The handler routes
// every request through the transpiled match table + per-controller
// Dispatch function (both emitted alongside this overlay file).
//
// A defer-recover catches framework-typed panics (RecordNotFoundError,
// RecordInvalidError) and translates them to the corresponding HTTP
// status code (404 / 422), mirroring Rails' ActiveRecord::RecordNotFound
// → 404 rescue convention. Untyped panics (genuine bugs) re-panic so
// the http.Server's per-request goroutine surfaces them as 500s with
// a stack trace.
func Router() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer func() {
			rec := recover()
			if rec == nil {
				return
			}
			switch e := rec.(type) {
			case *RecordNotFoundError:
				http.Error(w, e.Error(), http.StatusNotFound)
			case *RecordInvalidError:
				http.Error(w, e.Error(), http.StatusUnprocessableEntity)
			default:
				panic(rec) // genuine bug → 500 via net/http's per-goroutine recover
			}
		}()

		// .json suffix on the path is routed under the bare path
		// (e.g. `/articles.json` matches `/articles`), then the
		// dispatcher inspects the suffix to set RequestFormat.
		matchPath := strings.TrimSuffix(r.URL.Path, ".json")
		// Rack::MethodOverride: Rails' `button_to ..., method: :delete`
		// (and `form_with method: :patch`) render a POST form carrying a
		// `_method` hidden field, since browsers only natively issue
		// GET/POST. Honor it so the POST routes as the intended verb —
		// without this, `DELETE /articles/:id/comments/:id` never matches
		// and the destroy 404s. Only POST is overridable (matching Rack),
		// and only to a non-empty verb. `r.PostFormValue` calls
		// `ParseForm` internally; it is idempotent, so the later
		// `ParseFormParams` re-parse is a no-op.
		method := r.Method
		if method == http.MethodPost {
			if override := r.PostFormValue("_method"); override != "" {
				method = strings.ToUpper(override)
			}
		}
		m := ActionDispatchRouter_match(method, matchPath, RoutesTable)
		if m == nil {
			http.NotFound(w, r)
			return
		}
		body, status, contentType, location, flash := Dispatch(m.Controller, m.Action, m.PathParams, r)
		if location != "" {
			w.Header().Set("Location", location)
		}
		if contentType != "" {
			w.Header().Set("Content-Type", contentType)
		}
		// Carry the flash the action set into the next request (or clear
		// it once shown). Set-Cookie is a header, so this must precede
		// WriteHeader. Dispatch read the incoming flash from `r` and
		// returned `flash.ToPersisted()` — the show-once sweep already
		// happened in the shared Flash class.
		WriteFlashCookie(w, flash)
		if status == 0 {
			status = 200
		}
		w.WriteHeader(int(status))
		w.Write([]byte(body))
	})
}
