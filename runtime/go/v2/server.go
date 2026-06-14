// Roundhouse go2 server runtime.
//
// Hand-written, ships with the v2/ overlay. The emitted `main.go`
// template calls `Server_start(Router(), opts)` to open the
// production DB, apply schema, and run net/http. Mirrors
// `runtime/rust/server.rs` member-for-member at the start-time API
// (port/db_path defaults, schema-on-startup, env-var lookups);
// middleware (layout_wrap, method_override) lands in a later wedge.
//
// Function naming follows the go2 module-singleton convention
// (`Server_start`) so the bare-fn entry from `main.go` resolves
// without per-call shims.

package v2

import (
	"net/http"
	"net/url"
	"os"
)

// flashCookieName is the cookie that carries flash between the action
// that sets it (`redirect_to … notice:`) and the follow-on GET that
// renders it. Per browser by construction, so parallel sessions (e.g.
// Playwright workers) never share a flash slot — unlike a process-
// global store. The lifecycle ("show exactly once") lives in the shared
// Flash class (ActionDispatch::Flash#to_persisted); the server is only a
// storage adapter, mirroring runtime/typescript/server.ts.
const flashCookieName = "rh_flash"

// ReadFlashCookie decodes the persisted flash carried in the rh_flash
// cookie into the String-keyed map the Flash constructor reloads from
// (`NewActionDispatchFlash(persisted)`). Returns an empty map when the
// cookie is absent or unparseable — the first request in a session has
// no carried flash. Only the closed flash key set (notice/alert) is
// surfaced, matching the lowerer's recognized fields.
func ReadFlashCookie(r *http.Request) map[string]string {
	out := map[string]string{}
	c, err := r.Cookie(flashCookieName)
	if err != nil || c.Value == "" {
		return out
	}
	vals, err := url.ParseQuery(c.Value)
	if err != nil {
		return out
	}
	for _, k := range []string{"notice", "alert"} {
		if v := vals.Get(k); v != "" {
			out[k] = v
		}
	}
	return out
}

// WriteFlashCookie persists the entries the action SET this request —
// `Flash#to_persisted` has already swept the show-once entries, so an
// empty map means "nothing to carry forward". An empty map clears the
// cookie (MaxAge<0) so a notice shown once doesn't stick on the next
// page. Must be called before the response headers are flushed
// (Set-Cookie is a header), which router_glue.go does before WriteHeader.
func WriteFlashCookie(w http.ResponseWriter, persisted map[string]string) {
	if len(persisted) == 0 {
		http.SetCookie(w, &http.Cookie{
			Name:     flashCookieName,
			Value:    "",
			Path:     "/",
			MaxAge:   -1,
			HttpOnly: true,
		})
		return
	}
	vals := url.Values{}
	for k, v := range persisted {
		vals.Set(k, v)
	}
	http.SetCookie(w, &http.Cookie{
		Name:     flashCookieName,
		Value:    vals.Encode(),
		Path:     "/",
		HttpOnly: true,
	})
}

// StartOptions carries the per-process boot configuration. Defaults:
// db_path → ./storage/development.sqlite3, port → 3000 (or $PORT).
// SchemaSQL is required — typically the emitted `CreateTables`
// constant in `app/v2/schema_sql.go`.
type StartOptions struct {
	DBPath    string
	Port      string
	SchemaSQL string
}

// Server_start opens the production DB, applies the schema, and
// starts an HTTP listener. Blocks until ListenAndServe returns.
// Panics on listen failure — generated `main.go` is the only caller
// and a port-bind failure is unrecoverable.
func Server_start(handler http.Handler, opts StartOptions) {
	dbPath := opts.DBPath
	if dbPath == "" {
		if env := os.Getenv("DATABASE_PATH"); env != "" {
			dbPath = env
		} else {
			dbPath = "./storage/development.sqlite3"
		}
	}
	OpenProductionDB(dbPath, opts.SchemaSQL)

	port := opts.Port
	if port == "" {
		if env := os.Getenv("PORT"); env != "" {
			port = env
		} else {
			port = "3000"
		}
	}
	// Mount the Action Cable WebSocket endpoint alongside the app
	// handler. `/cable` upgrades to a WebSocket (cable.go); every
	// other path falls through to the transpiled router. Turbo's
	// default cable URL is `/cable`, matching CableHandler.
	mux := http.NewServeMux()
	// Serve compiled assets (tailwind.css, turbo.min.js, …) from
	// static/assets/ at /assets/* — the URLs the emitted layout's
	// stylesheet_link_tag / importmap reference. http.Dir blocks `..`
	// traversal. Mirrors runtime/rust/server.rs's ServeDir mount.
	mux.Handle("/assets/", http.StripPrefix("/assets/",
		http.FileServer(http.Dir("static/assets"))))
	mux.HandleFunc("/cable", CableHandler)
	mux.Handle("/", handler)

	addr := ":" + port
	if err := http.ListenAndServe(addr, mux); err != nil {
		panic("Server_start: " + err.Error())
	}
}
