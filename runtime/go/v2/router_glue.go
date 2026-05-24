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
func Router() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// .json suffix on the path is routed under the bare path
		// (e.g. `/articles.json` matches `/articles`), then the
		// dispatcher inspects the suffix to set RequestFormat.
		matchPath := strings.TrimSuffix(r.URL.Path, ".json")
		m := ActionDispatchRouter_match(r.Method, matchPath, RoutesTable)
		if m == nil {
			http.NotFound(w, r)
			return
		}
		body, status, contentType, location := Dispatch(m.Controller, m.Action, m.PathParams, r)
		if location != "" {
			w.Header().Set("Location", location)
		}
		if contentType != "" {
			w.Header().Set("Content-Type", contentType)
		}
		if status == 0 {
			status = 200
		}
		w.WriteHeader(int(status))
		w.Write([]byte(body))
	})
}
