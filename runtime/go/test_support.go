// Roundhouse Go test-support runtime.
//
// Hand-written, shipped alongside generated code (copied in by the
// Go emitter as `app/test_support.go`). Controller tests call into
// TestClient for pure in-process HTTP dispatch (no real server, no
// socket setup) and wrap responses in TestResponse for Rails-shaped
// assertions.
//
// Mirrors runtime/python/test_support.py and runtime/elixir/
// test_support.ex in intent and shape — substring-match on the
// response body, loose but good-enough for the scaffold blog's
// HTML.

package app

import (
	"strings"
	"testing"
)

// TestResponse wraps an ActionResponse with Rails-Minitest-shaped
// assertion helpers. Method names mirror the Ruby source; bodies
// substring-match for assert_select-style queries.
type TestResponse struct {
	t        *testing.T
	Body     string
	Status   int
	Location string
}

func newTestResponse(t *testing.T, raw ActionResponse) *TestResponse {
	t.Helper()
	status := raw.Status
	if status == 0 {
		status = 200
	}
	return &TestResponse{t: t, Body: raw.Body, Status: status, Location: raw.Location}
}

// AssertOk fails unless the status is 200. Mirrors `assert_response :success`.
func (r *TestResponse) AssertOk() {
	r.t.Helper()
	if r.Status != 200 {
		r.t.Fatalf("expected 200 OK, got %d", r.Status)
	}
}

// AssertUnprocessable fails unless the status is 422. Mirrors
// `assert_response :unprocessable_entity`.
func (r *TestResponse) AssertUnprocessable() {
	r.t.Helper()
	if r.Status != 422 {
		r.t.Fatalf("expected 422 Unprocessable Entity, got %d", r.Status)
	}
}

// AssertStatus fails unless the status equals `code`. Mirrors a
// numeric `assert_response`.
func (r *TestResponse) AssertStatus(code int) {
	r.t.Helper()
	if r.Status != code {
		r.t.Fatalf("expected status %d, got %d", code, r.Status)
	}
}

// AssertRedirectedTo asserts that the response is a 3xx and the
// Location header contains `path` as a substring.
func (r *TestResponse) AssertRedirectedTo(path string) {
	r.t.Helper()
	if r.Status < 300 || r.Status >= 400 {
		r.t.Fatalf("expected a redirection, got %d", r.Status)
	}
	if !strings.Contains(r.Location, path) {
		r.t.Fatalf("expected Location to contain %q, got %q", path, r.Location)
	}
}

// AssertSelect substring-matches a fragment derived from the
// selector against the response body. Compound selectors pick the
// first chunk; the rules match the TS / Python / Elixir twins.
func (r *TestResponse) AssertSelect(selector string) {
	r.t.Helper()
	frag := selectorFragment(selector)
	if !strings.Contains(r.Body, frag) {
		r.t.Fatalf("expected body to match selector %q (looked for %q)", selector, frag)
	}
}

// AssertSelectText combines AssertSelect with a substring text
// check. Mirrors `assert_select <sel>, <text>`.
func (r *TestResponse) AssertSelectText(selector, text string) {
	r.t.Helper()
	r.AssertSelect(selector)
	if !strings.Contains(r.Body, text) {
		r.t.Fatalf("expected body to contain text %q under selector %q", text, selector)
	}
}

// AssertSelectMin asserts that the selector fragment appears at
// least `n` times in the body. Mirrors `assert_select <sel>,
// minimum: n`.
func (r *TestResponse) AssertSelectMin(selector string, n int) {
	r.t.Helper()
	frag := selectorFragment(selector)
	count := strings.Count(r.Body, frag)
	if count < n {
		r.t.Fatalf("expected at least %d matches for selector %q, got %d", n, selector, count)
	}
}

func selectorFragment(selector string) string {
	first := strings.Fields(selector)
	if len(first) == 0 {
		return ""
	}
	head := first[0]
	switch {
	case strings.HasPrefix(head, "#"):
		return "id=\"" + head[1:] + "\""
	case strings.HasPrefix(head, "."):
		return head[1:] + "\""
	default:
		return "<" + head
	}
}

// TestClient is a pure in-process HTTP client — dispatches through
// Router.Match directly. No real HTTP, no socket setup.
type TestClient struct {
	t *testing.T
}

// NewTestClient returns a client bound to the given testing.T so
// dispatch failures (no route, etc.) fail the test.
func NewTestClient(t *testing.T) *TestClient {
	return &TestClient{t: t}
}

func (c *TestClient) Get(path string) *TestResponse {
	return c.dispatch("GET", path, nil)
}

func (c *TestClient) Post(path string, body map[string]string) *TestResponse {
	return c.dispatch("POST", path, body)
}

func (c *TestClient) Patch(path string, body map[string]string) *TestResponse {
	return c.dispatch("PATCH", path, body)
}

func (c *TestClient) Put(path string, body map[string]string) *TestResponse {
	return c.dispatch("PUT", path, body)
}

func (c *TestClient) Delete(path string) *TestResponse {
	return c.dispatch("DELETE", path, nil)
}

func (c *TestClient) dispatch(method, path string, body map[string]string) *TestResponse {
	c.t.Helper()
	handler, pathParams, ok := Router.Match(method, path)
	if !ok {
		c.t.Fatalf("no route for %s %s", method, path)
	}
	params := map[string]string{}
	for k, v := range pathParams {
		params[k] = v
	}
	for k, v := range body {
		params[k] = v
	}
	ctx := &ActionContext{Params: params}
	resp := handler(ctx)
	return newTestResponse(c.t, resp)
}
