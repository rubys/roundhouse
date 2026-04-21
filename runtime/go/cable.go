// Roundhouse Go cable runtime — scaffolding only.
//
// Parity with runtime/rust/cable.rs and runtime/python/cable.py is
// scoped to a later pass; for now `CableHandler` acknowledges the
// route exists so the server registration compiles, but doesn't
// upgrade the WebSocket. Client-side `@rails/actioncable` will
// attempt to connect and fail quietly — navigation and form-submit
// flows (which the compare tool exercises) don't depend on cable.
//
// The broadcaster + partial-renderer registry ports from railcar
// cleanly (`nhooyr.io/websocket` + an `actioncable-v1-json`
// subprotocol handshake); wiring happens when the Go emitter grows
// `broadcasts_to` output alongside the other targets.

package app

import (
	"net/http"
)

// CableHandler is the stub /cable endpoint. Returns 426 Upgrade
// Required so a curl probe sees a definitive status; a real browser
// client will interpret this as "cable unavailable" and move on.
func CableHandler(w http.ResponseWriter, r *http.Request) {
	_ = r
	http.Error(w, "WebSocket upgrade not wired yet", http.StatusUpgradeRequired)
}

// Broadcast is the stub broadcaster entry point. Generated models
// that register `broadcasts_to` will call through here. The later
// full port fans out to subscribers; for now it drops the payload.
func Broadcast(channel, body string) {
	_ = channel
	_ = body
}
