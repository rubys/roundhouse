// Roundhouse go2 form-params parser.
//
// Hand-written, ships with the v2/ overlay. Rails-style bracket
// notation in POST/PATCH/PUT form bodies (e.g. `article[title]=Hi`)
// expands to nested `map[string]RoundhouseParamValue` so transpiled
// `params["article"]["title"]` resolves correctly. Mirrors the same
// shape that path params and query params land in.
//
// Scope: x-www-form-urlencoded only — multipart bodies and JSON
// bodies are separate wedges if real-blog ever needs them.

package v2

import (
	"net/http"
	"strings"
)

// ParseFormParams reads r.Form (path-decoded application/x-www-form-
// urlencoded values) and expands Rails-style bracket-notation keys
// into nested maps. `article[title]=Foo` becomes
// `{"article": {"title": "Foo"}}`. Plain keys without brackets land
// as top-level string values. The first value for each key wins
// (matches Rails ActionController::Parameters semantics for repeated
// keys without `[]` suffix).
//
// The function is no-op safe for GET requests — `r.ParseForm()` only
// reads bodies on methods with content types it recognizes, and the
// query-string values fold into the same map (Rails parity).
func ParseFormParams(r *http.Request) map[string]any {
	out := map[string]any{}
	if err := r.ParseForm(); err != nil {
		return out
	}
	for key, values := range r.Form {
		if len(values) == 0 {
			continue
		}
		value := values[0]
		// Split `article[title][nested]` into ["article", "title", "nested"].
		// First segment is the bracket-free prefix; subsequent segments
		// are the bracketed parts.
		head, rest := splitBracketKey(key)
		if len(rest) == 0 {
			out[head] = value
			continue
		}
		assignNested(out, head, rest, value)
	}
	return out
}

// splitBracketKey returns the prefix and the bracketed segments.
// `article[title]` → ("article", ["title"]).
// `comment[author][name]` → ("comment", ["author", "name"]).
// `flat` → ("flat", []).
func splitBracketKey(key string) (string, []string) {
	idx := strings.Index(key, "[")
	if idx < 0 {
		return key, nil
	}
	head := key[:idx]
	tail := key[idx:]
	var segs []string
	for tail != "" {
		if !strings.HasPrefix(tail, "[") {
			break
		}
		end := strings.Index(tail, "]")
		if end < 0 {
			break
		}
		segs = append(segs, tail[1:end])
		tail = tail[end+1:]
	}
	return head, segs
}

// assignNested walks `head` then `rest` keys into `out`, creating
// intermediate `map[string]any` levels as needed, and writing `value`
// at the deepest level. Existing non-map values at intermediate
// positions are overwritten — matches what Rails does when a flat
// `article=…` later collides with `article[…]=…` (last-write wins).
func assignNested(out map[string]any, head string, rest []string, value string) {
	cur, ok := out[head].(map[string]any)
	if !ok {
		cur = map[string]any{}
		out[head] = cur
	}
	for i, seg := range rest {
		if i == len(rest)-1 {
			cur[seg] = value
			return
		}
		next, ok := cur[seg].(map[string]any)
		if !ok {
			next = map[string]any{}
			cur[seg] = next
		}
		cur = next
	}
}
