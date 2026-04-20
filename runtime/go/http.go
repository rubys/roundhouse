// Roundhouse Go HTTP runtime — Phase 4d pass-2 shape.
//
// Hand-written, shipped alongside generated code (copied in by the
// Go emitter as `app/http.go`). Provides controller-facing types +
// the Router match table; test_support.go calls Router.Match to
// dispatch.
//
// Mirrors runtime/python/http.py and runtime/elixir/http.ex in
// intent: ActionResponse/ActionContext value types, a package-level
// Router with a flat slice of routes registered at package init
// time. Go's static typing means we register handlers as direct
// function values rather than resolving by name (no reflection).

package app

import "strings"

// ActionResponse is what every generated controller action returns.
// Fields are optional so actions pick only what they need:
//
//	Body:     HTML string for GET actions
//	Status:   HTTP status code (default 200)
//	Location: redirect target URL (for 3xx responses)
type ActionResponse struct {
	Body     string
	Status   int
	Location string
}

// ActionContext is the per-request context handed to every action.
// Params merges path params (from the URL pattern) with form-body
// fields. Phase 4d uses a flat string→string map; richer typed
// access is a later phase.
type ActionContext struct {
	Params map[string]string
}

// Handler is the signature every generated action satisfies.
type Handler func(*ActionContext) ActionResponse

// Route is one registered method/path pair plus its handler. The
// router stores these in registration order so the emitter's flat
// route table determines match precedence.
type Route struct {
	Method  string
	Path    string
	Handler Handler
}

// Router is the package-wide route table. Generated `app/routes.go`
// pushes Route entries from its `init()` function; tests dispatch
// through `Router.Match`.
var Router = &routerImpl{}

type routerImpl struct {
	Routes []Route
}

// Reset clears the table — emitted `init()` calls this first so a
// re-init (e.g., after a `go test` rerun) doesn't accumulate.
func (r *routerImpl) Reset() {
	r.Routes = nil
}

func (r *routerImpl) Get(path string, h Handler)    { r.add("GET", path, h) }
func (r *routerImpl) Post(path string, h Handler)   { r.add("POST", path, h) }
func (r *routerImpl) Put(path string, h Handler)    { r.add("PUT", path, h) }
func (r *routerImpl) Patch(path string, h Handler)  { r.add("PATCH", path, h) }
func (r *routerImpl) Delete(path string, h Handler) { r.add("DELETE", path, h) }

func (r *routerImpl) add(method, path string, h Handler) {
	r.Routes = append(r.Routes, Route{Method: method, Path: path, Handler: h})
}

// Match resolves a (method, path) pair to a handler + path params.
// Used by TestClient; real HTTP dispatch is Phase 4e.
func (r *routerImpl) Match(method, path string) (Handler, map[string]string, bool) {
	for _, route := range r.Routes {
		if route.Method != method {
			continue
		}
		if params, ok := matchPath(route.Path, path); ok {
			return route.Handler, params, true
		}
	}
	return nil, nil, false
}

func matchPath(pattern, path string) (map[string]string, bool) {
	pp := splitPath(pattern)
	pv := splitPath(path)
	if len(pp) != len(pv) {
		return nil, false
	}
	params := map[string]string{}
	for i, seg := range pp {
		if strings.HasPrefix(seg, ":") {
			params[seg[1:]] = pv[i]
		} else if seg != pv[i] {
			return nil, false
		}
	}
	return params, true
}

func splitPath(p string) []string {
	parts := strings.Split(p, "/")
	out := parts[:0]
	for _, s := range parts {
		if s != "" {
			out = append(out, s)
		}
	}
	return out
}

// Phase 4c compile-only stubs ----------------------------------------
//
// Hand-written code outside the emitter may still call these. The
// emitter itself no longer generates calls to them — controller
// actions return ActionResponse directly.

// Response is the legacy stub return type. Kept so any older
// reference outside the emitter still type-checks.
type Response struct{}

type ParamSet struct{}

func (p *ParamSet) Expect(args ...interface{}) interface{} { return nil }
func (p *ParamSet) At(key interface{}) int64               { return 0 }

func Params() *ParamSet { return &ParamSet{} }

func Render(args ...interface{}) *Response     { return &Response{} }
func RedirectTo(args ...interface{}) *Response { return &Response{} }
func Head(args ...interface{}) *Response       { return &Response{} }

type FormatRouter struct{}

func (f *FormatRouter) Html(block func() *Response) *Response { return block() }
func (f *FormatRouter) Json(block func() *Response) *Response { return &Response{} }

func RespondTo(block func(*FormatRouter) *Response) *Response {
	return block(&FormatRouter{})
}
