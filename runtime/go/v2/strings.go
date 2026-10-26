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

// RhStrAt is Ruby's `str[i]`: the character at a character index, a
// negative one counting from the end; past either end Ruby answers
// nil, which the String-typed call answers as "".
func RhStrAt[I ~int | ~int64](s string, index I) string {
	return RhStrSlice(s, index, 1)
}

// RhSlice is Ruby's `ary[start, length]`: clamped to the array, a
// negative start counting from the end, and nil (a nil slice) for a
// start past the end.
func RhSlice[T any, S ~int | ~int64, L ~int | ~int64](xs []T, start S, length L) []T {
	n := int64(len(xs))
	b := int64(start)
	if b < 0 {
		b += n
	}
	l := int64(length)
	if b < 0 || b > n || l < 0 {
		return nil
	}
	e := b + l
	if e > n {
		e = n
	}
	out := make([]T, e-b)
	copy(out, xs[b:e])
	return out
}

// RhIndex is Ruby's `ary.index(x)`: the first position, or nil.
func RhIndex[T comparable](xs []T, x T) interface{} {
	for i, v := range xs {
		if v == x {
			return int64(i)
		}
	}
	return nil
}

// RhLast is Ruby's `ary.last`; an empty array answers the zero value
// that stands for nil.
func RhLast[T any](xs []T) T {
	var zero T
	if len(xs) == 0 {
		return zero
	}
	return xs[len(xs)-1]
}
