// Roundhouse go2 test-support runtime.
//
// Hand-written, include_str!'d by src/emit/go2.rs and emitted as
// `test_support_test.go`. Provides TestClient / TestResponse —
// dispatches through the same v2 router/controller path production
// traffic uses (so it exercises the real handler), and the Rails-
// Minitest-shaped assertion surface (AssertOk/AssertSelect/...) the
// lowered test bodies call. assert_select queries via the Dom
// primitive surface below.

package v2

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

// (SetupTestDB lives in db.go — it opens a fresh :memory: SQLite
// and runs schema DDL. fixtures_test.go's setupTest() calls it
// before reloading fixtures.)

// TestResponse — Rails-Minitest-shaped assertion helpers over a
// response captured via httptest.ResponseRecorder.
type TestResponse struct {
	t        *testing.T
	Body     string
	Status   int
	Location string
}

func (r *TestResponse) AssertOk() {
	r.t.Helper()
	if r.Status != 200 {
		r.t.Fatalf("expected 200 OK, got %d", r.Status)
	}
}

func (r *TestResponse) AssertUnprocessable() {
	r.t.Helper()
	if r.Status != 422 {
		r.t.Fatalf("expected 422 Unprocessable Entity, got %d", r.Status)
	}
}

func (r *TestResponse) AssertStatus(code int) {
	r.t.Helper()
	if r.Status != code {
		r.t.Fatalf("expected status %d, got %d", code, r.Status)
	}
}

func (r *TestResponse) AssertRedirectedTo(path string) {
	r.t.Helper()
	if r.Status < 300 || r.Status >= 400 {
		r.t.Fatalf("expected a redirection, got %d", r.Status)
	}
	if !strings.Contains(r.Location, path) {
		r.t.Fatalf("expected Location to contain %q, got %q", path, r.Location)
	}
}

// assert_select over the Dom primitive surface (defined below).
// Presence check: the selector matches at least one node. The stub
// Dom is a substring matcher, so this stays rough-but-effective for
// the scaffold-blog HTML shapes; a real engine tightens it without
// changing these sites.
func (r *TestResponse) AssertSelect(selector string) {
	r.t.Helper()
	if len(domSelect(domParse(r.Body), selector)) == 0 {
		r.t.Fatalf("expected body to match selector %q", selector)
	}
}

func (r *TestResponse) AssertSelectText(selector, text string) {
	r.t.Helper()
	nodes := domSelect(domParse(r.Body), selector)
	if len(nodes) == 0 {
		r.t.Fatalf("expected body to match selector %q", selector)
	}
	found := false
	for _, n := range nodes {
		if strings.Contains(domText(n), text) {
			found = true
			break
		}
	}
	if !found {
		r.t.Fatalf("expected text %q under selector %q", text, selector)
	}
}

func (r *TestResponse) AssertSelectMin(selector string, n int) {
	r.t.Helper()
	count := len(domSelect(domParse(r.Body), selector))
	if count < n {
		r.t.Fatalf("expected at least %d matches for selector %q, got %d", n, selector, count)
	}
}

// ── Dom primitive surface (the assert_select substrate) ────────────
//
// The HTML-query contract assert_select lowers to, shared in shape
// with the Ruby/TS/Python/Rust/Elixir twins (cross-target contract in
// runtime/spinel/test/test_helper.rbs). Stub: the substring matcher
// dressed as a Dom — domSelect fabricates one synthetic node (the
// whole document) per fragment occurrence and domText returns it
// verbatim, so presence / minimum / text checks degrade to exactly the
// pre-contract behavior. The upgrade path is to swap these three
// functions for a goquery-backed engine — real nodes, real CSS
// selectors — touching only this block; the TestResponse call sites
// stay put.

type domDoc = string
type domNode = string

// domParse parses an HTML document. Stub: the document *is* its html.
func domParse(html string) domDoc { return html }

// domSelect returns nodes matching selector within root (a document or
// node). Stub: one synthetic node (the root's html) per substring-
// fragment occurrence.
func domSelect(root domDoc, selector string) []domNode {
	frag := selectorFragment(selector)
	if frag == "" {
		return nil
	}
	var nodes []domNode
	from := 0
	for {
		i := strings.Index(root[from:], frag)
		if i < 0 {
			break
		}
		nodes = append(nodes, root)
		from += i + len(frag)
	}
	return nodes
}

// domText returns a node's concatenated descendant text. Stub: the
// node verbatim.
func domText(node domNode) string { return node }

// selectorFragment maps a loose selector to a substring fragment (the
// stub's rule, replaced by a real CSS engine on upgrade): "#id" →
// id="id", ".cls" → cls", "tag" → <tag. Compound selectors take the
// first chunk.
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

// TestClient — in-process HTTP client routed through Router() (the
// same handler production main.go boots). Uses httptest's
// ResponseRecorder so we don't open a real listener.
type TestClient struct {
	t *testing.T
}

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
	var bodyReader *strings.Reader
	if body != nil {
		form := url.Values{}
		for k, v := range body {
			form.Set(k, v)
		}
		bodyReader = strings.NewReader(form.Encode())
	}
	var req *http.Request
	if bodyReader != nil {
		req = httptest.NewRequest(method, path, bodyReader)
		req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	} else {
		req = httptest.NewRequest(method, path, nil)
	}
	w := httptest.NewRecorder()
	Router().ServeHTTP(w, req)
	return &TestResponse{
		t:        c.t,
		Body:     w.Body.String(),
		Status:   w.Code,
		Location: w.Header().Get("Location"),
	}
}
