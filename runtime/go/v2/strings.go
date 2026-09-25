// Ruby String semantics the emitter targets where Go's own operators
// differ.

package v2

// RhStrSlice is Ruby's `str[start, length]`: character-indexed, a
// negative start counting from the end, and CLAMPED to the string —
// `"abc"[1, 10]` is `"bc"`. Go's `s[a:a+n]` panics past the end and
// indexes bytes. Ruby answers nil for a start past the end; the
// emitted call is typed String, so that case answers "".
func RhStrSlice[S ~int | ~int64, L ~int | ~int64](s string, start S, length L) string {
	r := []rune(s)
	n := int64(len(r))
	b := int64(start)
	if b < 0 {
		b += n
	}
	l := int64(length)
	if b < 0 || b > n || l < 0 {
		return ""
	}
	e := b + l
	if e > n {
		e = n
	}
	return string(r[b:e])
}
