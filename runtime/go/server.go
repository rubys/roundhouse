// Roundhouse Go server runtime.
//
// Hand-written, shipped alongside generated code (copied in by the
// Go emitter as `app/server.go`). Uses net/http (stdlib) for HTTP
// dispatch — no external deps beyond modernc.org/sqlite which the
// DB runtime already requires.
//
// Dispatches through `Router.Match`, wraps HTML responses in the
// emitted layout when one is configured, and handles Rails'
// `_method=patch|put|delete` override convention.
//
// Mirrors runtime/rust/server.rs and runtime/python/server.py in
// intent: layout renderer passed via `StartOptions`, request-scoped
// yield/slot storage wiped at the start of each request.

package app

import (
	"bytes"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
)

// StartOptions bundles the few knobs that differ between dev and
// test (port, DB path) plus the emitted layout renderer. Mirrors
// the rust/TS runtimes' shape.
type StartOptions struct {
	// DBPath is the on-disk sqlite DB the server opens.
	// Defaults to `storage/development.sqlite3` when empty.
	DBPath string
	// Port defaults to 3000 or the PORT env var when zero.
	Port int
	// SchemaSQL is applied to the DB on startup when no tables
	// exist yet (the compare driver stages a pre-seeded DB, so
	// skipping on populated DBs preserves its fixtures).
	SchemaSQL string
	// Layout, when non-nil, wraps 2xx/422 HTML responses. The
	// action's body is stashed via SetYield before invoking.
	Layout func() string
}

// Start opens the DB, applies schema, and runs the HTTP server
// until the process exits. Blocks.
//
// Route plumbing mirrors runtime/rust/server.rs: `/cable` is
// registered as a dedicated WebSocket handler BEFORE the catchall,
// then everything else falls through to the Router-based HTTP
// dispatcher. Keeping the Router abstraction (rather than
// per-action `mux.HandleFunc` pattern entries the railcar
// generator produces) preserves the test-support dispatch path
// that runs without a live server.
func Start(opts StartOptions) {
	dbPath := opts.DBPath
	if dbPath == "" {
		dbPath = "storage/development.sqlite3"
	}
	port := opts.Port
	if port == 0 {
		if envPort := os.Getenv("PORT"); envPort != "" {
			if p, err := strconv.Atoi(envPort); err == nil {
				port = p
			}
		}
		if port == 0 {
			port = 3000
		}
	}

	OpenProductionDB(dbPath, opts.SchemaSQL)

	mux := http.NewServeMux()
	mux.HandleFunc("/cable", CableHandler)
	mux.HandleFunc("/", dispatchHandler(opts.Layout))

	addr := fmt.Sprintf("127.0.0.1:%d", port)
	log.SetFlags(0)
	log.SetOutput(os.Stdout)
	log.Printf("Roundhouse Go server listening on http://%s", addr)
	if err := http.ListenAndServe(addr, mux); err != nil {
		log.Fatalf("listen: %v", err)
	}
}

func dispatchHandler(layout func() string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Wipe yield/slot state before each request so the layout
		// doesn't see stale body from a prior dispatch.
		ResetRenderState()

		method := r.Method
		path := r.URL.Path

		bodyParams, rawBody, err := readFormBody(r)
		if err != nil {
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}
		_ = rawBody

		// Rails scaffold forms submit POST with `_method=patch|put|
		// delete` for verbs browsers don't support natively.
		// Rewrite before route lookup so the downstream handler
		// sees the true verb.
		if method == http.MethodPost {
			if override := strings.ToUpper(bodyParams["_method"]); override != "" {
				switch override {
				case "PATCH", "PUT", "DELETE":
					method = override
				}
			}
		}

		handler, pathParams, ok := Router.Match(method, path)
		if !ok {
			http.Error(w, "Not Found", http.StatusNotFound)
			return
		}
		params := map[string]string{}
		for k, v := range pathParams {
			params[k] = v
		}
		for k, v := range bodyParams {
			params[k] = v
		}

		ctx := &ActionContext{Params: params}
		resp := handler(ctx)
		status := resp.Status
		if status == 0 {
			status = 200
		}

		if status >= 300 && status < 400 && resp.Location != "" {
			w.Header().Set("Location", resp.Location)
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			w.WriteHeader(status)
			_, _ = io.WriteString(w, resp.Body)
			return
		}

		body := resp.Body
		if layout != nil {
			SetYield(body)
			body = layout()
		}
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.WriteHeader(status)
		_, _ = io.WriteString(w, body)
	}
}

// readFormBody parses an urlencoded request body into a flat
// `string → string` map. Rails' bracket-notation (`article[title]=
// foo`) is passed through as-is; the emitted controllers look up
// `params["article[title]"]` rather than traversing a nested tree.
func readFormBody(r *http.Request) (map[string]string, []byte, error) {
	out := map[string]string{}
	if r.Body == nil {
		return out, nil, nil
	}
	ct := r.Header.Get("Content-Type")
	if !strings.HasPrefix(ct, "application/x-www-form-urlencoded") {
		// Drain so the connection can be reused.
		_, _ = io.Copy(io.Discard, r.Body)
		return out, nil, nil
	}
	var buf bytes.Buffer
	if _, err := io.Copy(&buf, r.Body); err != nil {
		return nil, nil, err
	}
	raw := buf.Bytes()
	values, err := url.ParseQuery(string(raw))
	if err != nil {
		return nil, raw, err
	}
	for k, vs := range values {
		if len(vs) > 0 {
			out[k] = vs[len(vs)-1]
		}
	}
	return out, raw, nil
}
