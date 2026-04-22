// Roundhouse Go cable runtime.
//
// Action Cable WebSocket + Turbo Streams broadcaster. Mirrors
// runtime/rust/cable.rs + runtime/python/cable.py — same wire
// format (actioncable-v1-json), same partial-renderer registry,
// same per-channel subscriber map.
//
// Uses nhooyr.io/websocket — added to go.mod by the emitter and
// resolved via `go mod tidy` on first build.

package app

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"strings"
	"sync"
	"time"

	"nhooyr.io/websocket"
)

// ── Partial-renderer registry ─────────────────────────────────

var (
	partialMu        sync.RWMutex
	partialRenderers = map[string]func(int64) string{}
)

// RegisterPartial associates a model class name with a renderer
// that takes a record id and returns the partial HTML. Cable's
// broadcast_*_to helpers consult this registry to produce frames
// without needing model-specific imports at the cable-runtime
// layer.
func RegisterPartial(typeName string, fn func(int64) string) {
	partialMu.Lock()
	defer partialMu.Unlock()
	partialRenderers[typeName] = fn
}

// RenderPartial looks up and invokes a registered renderer.
// Returns a placeholder div when no renderer is registered so
// callers never panic on an unknown type.
func RenderPartial(typeName string, id int64) string {
	partialMu.RLock()
	fn := partialRenderers[typeName]
	partialMu.RUnlock()
	if fn == nil {
		return "<div>" + typeName + " #" + itoa64(id) + "</div>"
	}
	return fn(id)
}

// ── Turbo Streams rendering ───────────────────────────────────

// TurboStreamHTML wraps a rendered partial in the Turbo Stream
// element Turbo's client splices into the DOM. Empty content
// collapses to a self-closing template (used by `remove`).
func TurboStreamHTML(action, target, content string) string {
	if content == "" {
		return `<turbo-stream action="` + action + `" target="` + target + `"></turbo-stream>`
	}
	return `<turbo-stream action="` + action + `" target="` + target + `"><template>` + content + `</template></turbo-stream>`
}

func domIDFor(table string, id int64) string {
	singular := table
	if strings.HasSuffix(singular, "s") {
		singular = singular[:len(singular)-1]
	}
	return singular + "_" + itoa64(id)
}

// ── Broadcast helpers ─────────────────────────────────────────

// BroadcastReplaceTo replaces the target element with the record's
// partial. Empty target defaults to `<singular>_<id>`.
func BroadcastReplaceTo(table string, id int64, typeName, channel, target string) {
	t := target
	if t == "" {
		t = domIDFor(table, id)
	}
	html := RenderPartial(typeName, id)
	dispatch(channel, TurboStreamHTML("replace", t, html))
}

// BroadcastPrependTo prepends the partial into the target
// container. Empty target defaults to the table name (the scaffold
// convention: `<ul id="articles">`).
func BroadcastPrependTo(table string, id int64, typeName, channel, target string) {
	t := target
	if t == "" {
		t = table
	}
	html := RenderPartial(typeName, id)
	dispatch(channel, TurboStreamHTML("prepend", t, html))
}

// BroadcastAppendTo appends the partial into the target container.
func BroadcastAppendTo(table string, id int64, typeName, channel, target string) {
	t := target
	if t == "" {
		t = table
	}
	html := RenderPartial(typeName, id)
	dispatch(channel, TurboStreamHTML("append", t, html))
}

// BroadcastRemoveTo removes the target element. Empty target
// defaults to `<singular>_<id>`.
func BroadcastRemoveTo(table string, id int64, channel, target string) {
	t := target
	if t == "" {
		t = domIDFor(table, id)
	}
	dispatch(channel, TurboStreamHTML("remove", t, ""))
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

// Broadcast is the stub kept for back-compat with the earlier
// server wiring — the `/cable` handler now upgrades.
func Broadcast(channel, html string) { dispatch(channel, html) }

// ── WebSocket handler ─────────────────────────────────────────

// CableHandler is mounted at `/cable` by the server. It
// negotiates the actioncable-v1-json subprotocol, sends welcome +
// ping frames, handles subscribe commands, and cleans up
// subscriptions on close.
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

	// Ping loop.
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

// decodeChannel recovers the channel name from Turbo's signed
// stream identifier. The identifier is a JSON blob like
// `{"channel":"Turbo::StreamsChannel",
//
//	"signed_stream_name":"<base64>--<digest>"}`;
//
// the base64 prefix holds a JSON-encoded channel name. Returns ""
// on malformed input so the handler silently drops the frame.
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

// itoa64 is a no-alloc local int64→string since strconv.FormatInt
// is in strconv which the cable runtime doesn't otherwise need.
func itoa64(n int64) string {
	if n == 0 {
		return "0"
	}
	var buf [20]byte
	i := len(buf)
	neg := n < 0
	if neg {
		n = -n
	}
	for n > 0 {
		i--
		buf[i] = byte('0' + n%10)
		n /= 10
	}
	if neg {
		i--
		buf[i] = '-'
	}
	return string(buf[i:])
}
