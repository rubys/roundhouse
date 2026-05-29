// Roundhouse go2 Turbo Streams broadcasts shim.
//
// The model lowerer's `broadcasts_to` expansion (see
// `src/lower/broadcasts.rs`) produces calls like
// `Broadcasts.prepend(stream: "x", target: "y", html: "...")` from
// inside model callback methods (`after_create_commit`, etc.). go2
// emits each call as `Broadcasts_<action>(map[string]interface{}{…})`
// — the bare-fn module-singleton shape — so this file provides one
// `Broadcasts_<action>` per Ruby `def self.<action>`.
//
// State is a process-wide log mutex so framework tests can assert
// what got emitted. Production wiring (Cable websocket fan-out)
// isn't here yet — mirrors rust2's Phase 5 stub stance. Each
// per-target broadcasts shim (runtime/rust/broadcasts.rs,
// runtime/crystal/broadcasts.cr) is hand-written for the same
// reason: no useful Ruby implementation exists outside
// runtime/spinel/broadcasts.rb (which depends on Ruby-specific
// module-level Array mutation that doesn't translate cleanly to
// Go's package-var semantics).

package v2

import (
	"fmt"
	"sync"
)

// BroadcastEntry mirrors the rust2 tuple shape. Order matches
// emission order so test assertions can stage check action-by-action.
type BroadcastEntry struct {
	Action string
	Stream string
	Target string
	HTML   string
}

var (
	broadcastsMu  sync.Mutex
	broadcastsLog []BroadcastEntry
)

// Broadcasts_reset_log_bang clears the in-memory log. Framework tests
// call this between assertions; production typically doesn't. The
// `_bang` suffix is the go2 emitter's mapping of Ruby's `!` marker.
func Broadcasts_reset_log_bang() {
	broadcastsMu.Lock()
	defer broadcastsMu.Unlock()
	broadcastsLog = nil
}

// Broadcasts_log returns a snapshot of the log. Returns a fresh slice
// so callers can iterate without holding the mutex.
func Broadcasts_log() []BroadcastEntry {
	broadcastsMu.Lock()
	defer broadcastsMu.Unlock()
	out := make([]BroadcastEntry, len(broadcastsLog))
	copy(out, broadcastsLog)
	return out
}

func Broadcasts_append(attrs map[string]interface{}) {
	recordBroadcast("append", attrs)
}

func Broadcasts_prepend(attrs map[string]interface{}) {
	recordBroadcast("prepend", attrs)
}

func Broadcasts_replace(attrs map[string]interface{}) {
	recordBroadcast("replace", attrs)
}

func Broadcasts_remove(attrs map[string]interface{}) {
	recordBroadcast("remove", attrs)
}

func recordBroadcast(action string, attrs map[string]interface{}) {
	entry := BroadcastEntry{
		Action: action,
		Stream: anyToString(attrs["stream"]),
		Target: anyToString(attrs["target"]),
		HTML:   anyToString(attrs["html"]),
	}
	broadcastsMu.Lock()
	broadcastsLog = append(broadcastsLog, entry)
	broadcastsMu.Unlock()

	// Live fan-out: compose the <turbo-stream> wrapper and push it to
	// every WebSocket subscriber on this stream (cable.go). The log
	// append above stays the test-visible contract; this is the
	// live-server contract. dispatch is a no-op when nothing is
	// subscribed, so non-cable scenarios (tests, one-shot runs) are
	// unaffected. Done outside the broadcastsMu critical section —
	// cable.go owns its own subscriber mutex.
	dispatch(entry.Stream, TurboStreamHTML(entry.Action, entry.Target, entry.HTML))
}

func anyToString(v any) string {
	if v == nil {
		return ""
	}
	if s, ok := v.(string); ok {
		return s
	}
	return fmt.Sprintf("%v", v)
}
