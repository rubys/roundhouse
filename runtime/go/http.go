// Roundhouse Go HTTP runtime — Phase 4c compile-only stubs.
//
// Hand-written, shipped alongside generated code (copied in by the Go
// emitter as `app/http.go`). Provides just enough surface that
// emitted controller actions type-check: `Response`, a `Params`
// placeholder, and the package-level functions generated code expects
// (`Render`, `RedirectTo`, `Head`, `RespondTo`).
//
// Mirrors `runtime/rust/http.rs` and `runtime/crystal/http.cr`. Every
// call returns `&Response{}`. Real behavior is Phase 4e+. Controller
// tests stay `t.Skip`-ped, so nothing actually executes during
// `go test`; the purpose is to make `go vet ./app` and `go build ./app`
// succeed.

package app

// Response is the opaque return value for every controller action.
// Real runtime will carry status + body + headers; Phase 4c only
// needs a value every action can return.
type Response struct{}

// Params stands in for the request's parameter parser. Bare `params`
// in a Ruby controller lowers to `Params()` — both reads and the
// `params.expect(...)` surface live on this stub.
type ParamSet struct{}

// Expect is a placeholder for `params.expect(:key)` /
// `params.expect(article: [...])`. Accepts any arg shape; returns
// `nil` so the caller's `interface{}`-typed consumers accept it. The
// real runtime will type-check and coerce.
func (p *ParamSet) Expect(args ...interface{}) interface{} {
	return nil
}

// At is the Go-compatible form of Ruby's `params[:id]` / `params["id"]`.
// Returns an int64 zero so the common call site `ArticleFind(Params().
// At("id"))` typechecks.
func (p *ParamSet) At(key interface{}) int64 {
	return 0
}

// Params returns a fresh parameter set. Stub — the real runtime would
// hand back the per-request one.
func Params() *ParamSet {
	return &ParamSet{}
}

// Render, RedirectTo, Head accept any positional arg shape the emitter
// produces (template symbol, a string, a model, an options map). All
// ignore their args and return an empty Response.
func Render(args ...interface{}) *Response {
	return &Response{}
}

func RedirectTo(args ...interface{}) *Response {
	return &Response{}
}

func Head(args ...interface{}) *Response {
	return &Response{}
}

// FormatRouter is the receiver in a `respond_to do |format| ... end`
// block. Phase 4c wires only the HTML branch; the JSON branch is
// replaced at the call site with a `// TODO: JSON branch` comment.
type FormatRouter struct{}

func (f *FormatRouter) Html(block func() *Response) *Response {
	return block()
}

func (f *FormatRouter) Json(block func() *Response) *Response {
	return &Response{}
}

// RespondTo runs the block with a fresh FormatRouter and surfaces the
// Response it produced.
func RespondTo(block func(*FormatRouter) *Response) *Response {
	return block(&FormatRouter{})
}
