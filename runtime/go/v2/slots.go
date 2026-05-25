// Per-request slot store for content_for / yield in view rendering.
//
// Replaces the prior transpile-time emit of a package-level
// `ActionViewViewHelpers_slots_slot` map + RWMutex (see commit
// 1f2a984). That fix made the concurrent-map-access panic go away but
// left the data-race semantics intact: thread A's
// `content_for(:title, ...)` could be overwritten by thread B
// between A's set and A's yield(:title). Per-goroutine scoping
// eliminates that race for the goroutine-per-request server model
// (which is what app/v2/main.go's net/http handler uses).
//
// The goroutine ID is parsed from runtime.Stack output — a
// documented Go antipattern. The right fix architecturally is
// threading a *Slots through every view function signature; see
// runtime/ruby/action_view/slots.rb + issue #7 for the cross-target
// per-request scope refactor that retires this file. Until then,
// this is a contained workaround: ~5 slot calls per request, each
// ~1µs of runtime.Stack parsing — in the noise vs. HTML rendering.
//
// Memory bound: the sync.Map keyed by goroutine ID is overwritten on
// every dispatch entry (via ActionViewViewHelpers_reset_slots_bang),
// so size stays at the live-goroutine count. Goroutine IDs are
// reused as net/http pools workers — the next dispatch on a recycled
// goroutine overwrites the stale entry rather than appending.

package v2

import (
	"bytes"
	"runtime"
	"strconv"
	"sync"
)

type slotsStore struct {
	data map[string]string
}

func newSlotsStore() *slotsStore {
	return &slotsStore{data: map[string]string{}}
}

var slotsByGoroutine sync.Map // uint64 → *slotsStore

func currentSlots() *slotsStore {
	gid := goroutineID()
	if v, ok := slotsByGoroutine.Load(gid); ok {
		return v.(*slotsStore)
	}
	// Fallback for callers outside an HTTP dispatch (e.g. a test
	// that directly invokes a view-helper slot getter). Synthesize
	// a fresh store on the fly so the call doesn't nil-panic; the
	// next reset_slots_bang on this goroutine overwrites it.
	s := newSlotsStore()
	slotsByGoroutine.Store(gid, s)
	return s
}

func goroutineID() uint64 {
	var buf [64]byte
	n := runtime.Stack(buf[:], false)
	// runtime.Stack writes lines like "goroutine 17 [running]:\n…".
	// First token is "goroutine"; second token is the ID.
	s := buf[:n]
	s = bytes.TrimPrefix(s, []byte("goroutine "))
	if i := bytes.IndexByte(s, ' '); i >= 0 {
		s = s[:i]
	}
	id, _ := strconv.ParseUint(string(s), 10, 64)
	return id
}

// The six functions below replace the transpile-emitted slot methods
// on ActionView::ViewHelpers. The Go emit suppresses those methods +
// the `@slots` ivar declaration in src/emit/go2.rs::format_module_ivar
// and src/emit/go2.rs::go_units_filter so this file owns them.

func ActionViewViewHelpers_reset_slots_bang() {
	slotsByGoroutine.Store(goroutineID(), newSlotsStore())
}

func ActionViewViewHelpers_content_for_set(slot string, value string) {
	currentSlots().data[slot] = value
}

func ActionViewViewHelpers_content_for_get(slot string) string {
	return currentSlots().data[slot]
}

func ActionViewViewHelpers_get_slot(slot string) string {
	return currentSlots().data[slot]
}

func ActionViewViewHelpers_get_yield() string {
	return currentSlots().data["__body__"]
}

func ActionViewViewHelpers_set_yield(content string) {
	currentSlots().data["__body__"] = content
}
