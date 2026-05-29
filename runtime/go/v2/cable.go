// Roundhouse go2 cable runtime.
//
// Action Cable WebSocket + Turbo Streams broadcaster. Mirrors
// runtime/rust/cable.rs + runtime/crystal/cable.cr — same wire
// format (actioncable-v1-json), same per-channel subscriber map.
//
// Unlike the retired legacy `runtime/go/cable.go`, this version has
// no partial-renderer registry: the `broadcasts_to` lowering renders
// the fragment HTML inside the model callback and hands it to
// `Broadcasts_<action>` already-rendered, so `recordBroadcast`
// (broadcasts.go) composes the `<turbo-stream>` wrapper and calls
// `dispatch` directly. The channel key a broadcast fans out on is the
// `stream` string; the subscriber side recovers the same string from
// Turbo's signed-stream-name via decodeChannel.
//
// Uses github.com/coder/websocket (the maintained successor to
// nhooyr.io/websocket) — added to go.mod by the emitter and resolved
// via `go mod tidy` on first build.

package v2

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/coder/websocket"
)

// ── Turbo Streams rendering ───────────────────────────────────

// TurboStreamHTML wraps rendered content in the Turbo Stream element
// Turbo's client splices into the DOM. Empty content collapses to a
// self-closing element (used by `remove`). Matches the spinel
// runtime's Broadcasts.render_fragment.
func TurboStreamHTML(action, target, content string) string {
	if content == "" {
		return `<turbo-stream action="` + action + `" target="` + target + `"></turbo-stream>`
	}
	return `<turbo-stream action="` + action + `" target="` + target + `"><template>` + content + `</template></turbo-stream>`
}

// ── Subscriber registry + dispatch ────────────────────────────

type cableSub struct {
	conn       *websocket.Conn
	identifier string
}

var (
	subsMu      sync.Mutex
	subscribers = map[string][]*cableSub{}
)

// dispatch fans `html` out to every subscriber on `channel` wrapped
// in the Action Cable message envelope Turbo expects. Called from
// recordBroadcast (broadcasts.go) on every model after-commit hook.
func dispatch(channel, html string) {
	subsMu.Lock()
	subs := append([]*cableSub(nil), subscribers[channel]...)
	subsMu.Unlock()
	for _, s := range subs {
		payload, err := json.Marshal(map[string]any{
			"type":       "message",
			"identifier": s.identifier,
			"message":    html,
		})
		if err != nil {
			continue
		}
		// Best-effort write with a short timeout so a stalled
		// client doesn't block the broadcaster.
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		_ = s.conn.Write(ctx, websocket.MessageText, payload)
		cancel()
	}
}

// ── WebSocket handler ─────────────────────────────────────────

// CableHandler is mounted at `/cable` by Server_start. It negotiates
// the actioncable-v1-json subprotocol, sends welcome + 3s ping
// frames, handles subscribe commands, and cleans up subscriptions on
// close.
func CableHandler(w http.ResponseWriter, r *http.Request) {
	conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{
		Subprotocols: []string{"actioncable-v1-json"},
	})
	if err != nil {
		return
	}
	defer conn.Close(websocket.StatusInternalError, "cable handler exiting")

	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()

	// Welcome frame.
	welcome, _ := json.Marshal(map[string]string{"type": "welcome"})
	if err := conn.Write(ctx, websocket.MessageText, welcome); err != nil {
		return
	}

	// Ping loop. Turbo's client treats the absence of pings (~6s) as
	// a dead connection and reconnects, so this is required.
	go func() {
		ticker := time.NewTicker(3 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				payload, _ := json.Marshal(map[string]any{
					"type":    "ping",
					"message": time.Now().Unix(),
				})
				wctx, wcancel := context.WithTimeout(ctx, 2*time.Second)
				err := conn.Write(wctx, websocket.MessageText, payload)
				wcancel()
				if err != nil {
					return
				}
			}
		}
	}()

	var (
		myMu  sync.Mutex
		myEnt []struct {
			channel string
			sub     *cableSub
		}
	)

	// Subscribe handler loop.
	for {
		_, data, err := conn.Read(ctx)
		if err != nil {
			break
		}
		var payload map[string]any
		if err := json.Unmarshal(data, &payload); err != nil {
			continue
		}
		cmd, _ := payload["command"].(string)
		if cmd != "subscribe" {
			continue
		}
		identifier, _ := payload["identifier"].(string)
		if identifier == "" {
			continue
		}
		channel := decodeChannel(identifier)
		if channel == "" {
			continue
		}
		sub := &cableSub{conn: conn, identifier: identifier}
		subsMu.Lock()
		subscribers[channel] = append(subscribers[channel], sub)
		subsMu.Unlock()
		myMu.Lock()
		myEnt = append(myEnt, struct {
			channel string
			sub     *cableSub
		}{channel, sub})
		myMu.Unlock()
		confirm, _ := json.Marshal(map[string]string{
			"type":       "confirm_subscription",
			"identifier": identifier,
		})
		_ = conn.Write(ctx, websocket.MessageText, confirm)
	}

	// Cleanup on disconnect.
	myMu.Lock()
	ents := myEnt
	myMu.Unlock()
	subsMu.Lock()
	for _, e := range ents {
		list := subscribers[e.channel]
		for i, s := range list {
			if s == e.sub {
				subscribers[e.channel] = append(list[:i], list[i+1:]...)
				break
			}
		}
		if len(subscribers[e.channel]) == 0 {
			delete(subscribers, e.channel)
		}
	}
	subsMu.Unlock()
	conn.Close(websocket.StatusNormalClosure, "")
}

// decodeChannel recovers the channel/stream name from Turbo's signed
// stream identifier. The identifier is a JSON blob like
// `{"channel":"Turbo::StreamsChannel",
//
//	"signed_stream_name":"<base64>--<digest>"}`;
//
// the base64 prefix holds a JSON-encoded stream name (the same value
// the `stream` field of a broadcast carries). Returns "" on malformed
// input so the handler silently drops the frame.
func decodeChannel(identifier string) string {
	var id map[string]any
	if err := json.Unmarshal([]byte(identifier), &id); err != nil {
		return ""
	}
	signed, ok := id["signed_stream_name"].(string)
	if !ok {
		return ""
	}
	b64 := signed
	if idx := strings.Index(signed, "--"); idx >= 0 {
		b64 = signed[:idx]
	}
	decoded, err := base64.StdEncoding.DecodeString(b64)
	if err != nil {
		return ""
	}
	var channel string
	if err := json.Unmarshal(decoded, &channel); err != nil {
		return ""
	}
	return channel
}
