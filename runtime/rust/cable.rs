//! Action Cable server + Turbo Streams broadcaster.
//!
//! Mirrors the TS runtime's `CableServer` + `/cable` handler.
//! Ported from railcar's proven implementation (pings + the
//! `actioncable-v1-json` subprotocol are both known-good there).
//!
//! Two halves:
//!   1. Broadcasting — models call `broadcast_prepend_to` /
//!      `broadcast_replace_to` / `broadcast_remove_to`, which render
//!      the appropriate `<turbo-stream>` element and push it to
//!      every subscriber of the given channel. A partial-renderer
//!      registry (`register_partial`) lets the runtime reach back
//!      into the generated `views::*` functions without this file
//!      having to know model-specific types.
//!   2. WebSocket — `cable_handler` upgrades incoming requests,
//!      sends a welcome frame, pings every 3s, and on `subscribe`
//!      commands decodes Turbo's signed-stream-name blob to recover
//!      the channel name, then registers the socket's outbound mpsc
//!      sender with the global `CABLE` registry.
//!
//! Generated code (Phase B of the cable work) calls these via
//! `crate::cable::...` from `impl Broadcaster for <Model>` blocks
//! produced by the emitter's `broadcasts_to` translation.
//!
//! The `actioncable-v1-json` subprotocol spec:
//!   https://github.com/rails/rails/blob/main/actioncable/lib/action_cable/server/worker.rb
//! Frames used here: `welcome`, `ping`, `confirm_subscription`,
//! `message`. Rejections + `unsubscribe` commands aren't needed
//! for the current broadcast paths.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;

// ── Partial-renderer registry ──────────────────────────────────
//
// Models register a closure that renders an instance identified by
// id into its Turbo Stream partial HTML. Kept as a runtime lookup
// (rather than parameterising Broadcaster on the model type) so
// broadcasts called on associations — e.g., `comment.article`'s
// replace broadcast — can find the parent's partial without the
// child model needing to know the parent's view module.

pub type RenderPartialFn = Box<dyn Fn(i64) -> String + Send + Sync>;

static PARTIAL_RENDERERS: LazyLock<RwLock<HashMap<String, RenderPartialFn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a partial renderer for `type_name` (the model class
/// name, e.g. `"Article"`). The closure receives a record id and
/// returns the rendered partial HTML, or empty string on miss.
pub fn register_partial(type_name: &str, f: impl Fn(i64) -> String + Send + Sync + 'static) {
    PARTIAL_RENDERERS
        .write()
        .expect("cable partial renderers poisoned")
        .insert(type_name.to_string(), Box::new(f));
}

/// Look up and invoke a registered partial renderer. Returns a
/// placeholder div when no renderer is registered — tests can
/// assert on the fallback rather than panicking.
pub fn render_partial(type_name: &str, id: i64) -> String {
    let table = PARTIAL_RENDERERS
        .read()
        .expect("cable partial renderers poisoned");
    match table.get(type_name) {
        Some(f) => f(id),
        None => format!("<div>{} #{}</div>", type_name, id),
    }
}

// ── Turbo Streams rendering ────────────────────────────────────

/// Render a single `<turbo-stream>` element. Empty content collapses
/// to a self-closing template (used by `remove` actions).
pub fn turbo_stream_html(action: &str, target: &str, content: &str) -> String {
    if content.is_empty() {
        format!(
            r#"<turbo-stream action="{}" target="{}"></turbo-stream>"#,
            action, target
        )
    } else {
        format!(
            r#"<turbo-stream action="{}" target="{}"><template>{}</template></turbo-stream>"#,
            action, target, content,
        )
    }
}

/// Rails convention: `<singular>_<id>`. Naive depluralise — strips
/// a trailing `s` if present. Matches railcar's `dom_id_for`.
fn dom_id_for(table_name: &str, id: i64) -> String {
    let singular = table_name.strip_suffix('s').unwrap_or(table_name);
    format!("{}_{}", singular, id)
}

// ── Broadcast helpers ──────────────────────────────────────────

/// Replace the target element with the record's partial. Defaults
/// `target` to `<singular>_<id>` when caller passes an empty
/// string (matches Rails' `broadcast_replace_to` with no explicit
/// target).
pub fn broadcast_replace_to(
    table_name: &str,
    id: i64,
    type_name: &str,
    channel: &str,
    target: &str,
) {
    let target = if target.is_empty() {
        dom_id_for(table_name, id)
    } else {
        target.to_string()
    };
    let html = render_partial(type_name, id);
    let stream = turbo_stream_html("replace", &target, &html);
    CABLE.broadcast(channel, &stream);
}

/// Prepend the record's partial into the target container.
/// Defaults `target` to the table name (the scaffold's `<ul
/// id="articles">` convention).
pub fn broadcast_prepend_to(
    table_name: &str,
    id: i64,
    type_name: &str,
    channel: &str,
    target: &str,
) {
    let target = if target.is_empty() {
        table_name.to_string()
    } else {
        target.to_string()
    };
    let html = render_partial(type_name, id);
    let stream = turbo_stream_html("prepend", &target, &html);
    CABLE.broadcast(channel, &stream);
}

/// Append the record's partial into the target container. Same
/// default-target rule as prepend.
pub fn broadcast_append_to(
    table_name: &str,
    id: i64,
    type_name: &str,
    channel: &str,
    target: &str,
) {
    let target = if target.is_empty() {
        table_name.to_string()
    } else {
        target.to_string()
    };
    let html = render_partial(type_name, id);
    let stream = turbo_stream_html("append", &target, &html);
    CABLE.broadcast(channel, &stream);
}

/// Remove the target element. Target defaults to `<singular>_<id>`
/// so `broadcast_remove_to(channel)` on a record deletes its own
/// DOM node.
pub fn broadcast_remove_to(table_name: &str, id: i64, channel: &str, target: &str) {
    let target = if target.is_empty() {
        dom_id_for(table_name, id)
    } else {
        target.to_string()
    };
    let stream = turbo_stream_html("remove", &target, "");
    CABLE.broadcast(channel, &stream);
}

// ── CableServer ────────────────────────────────────────────────

struct Subscriber {
    /// Outbound channel to this socket's send task. Cloned into the
    /// registry; the socket task owns the receiver half.
    tx: mpsc::UnboundedSender<String>,
    /// The raw identifier string the client sent on subscribe. We
    /// echo it back in every broadcast so Turbo can route the
    /// message to the right `<turbo-cable-stream-source>` element.
    identifier: String,
}

pub struct CableServer {
    channels: RwLock<HashMap<String, Vec<Subscriber>>>,
}

/// Process-wide registry. One per server; fine as a static because
/// the server runs one app per process (same assumption as
/// `server::LAYOUT_FN`).
pub static CABLE: LazyLock<CableServer> = LazyLock::new(|| CableServer {
    channels: RwLock::new(HashMap::new()),
});

impl CableServer {
    fn subscribe(&self, channel: &str, tx: mpsc::UnboundedSender<String>, identifier: &str) {
        self.channels
            .write()
            .expect("cable channels poisoned")
            .entry(channel.to_string())
            .or_default()
            .push(Subscriber {
                tx,
                identifier: identifier.to_string(),
            });
    }

    /// Drop any subscribers whose `tx` matches the given pointer.
    /// Called on socket close. We compare by pointer rather than
    /// PartialEq because `UnboundedSender` doesn't implement it;
    /// each subscriber holds a distinct sender, so pointer identity
    /// is sufficient and avoids threading a subscriber id through
    /// the WebSocket task.
    fn unsubscribe(&self, tx_ptr: usize) {
        let mut channels = self.channels.write().expect("cable channels poisoned");
        for subs in channels.values_mut() {
            subs.retain(|s| &s.tx as *const _ as usize != tx_ptr);
        }
        channels.retain(|_, subs| !subs.is_empty());
    }

    /// Push `html` as a Turbo Stream `message` frame to every
    /// subscriber on `channel`. Dropped senders are ignored — the
    /// subscriber will be cleaned up on the close-driven
    /// `unsubscribe` path.
    pub fn broadcast(&self, channel: &str, html: &str) {
        let channels = self.channels.read().expect("cable channels poisoned");
        if let Some(subs) = channels.get(channel) {
            for sub in subs {
                let frame = json!({
                    "type": "message",
                    "identifier": sub.identifier,
                    "message": html,
                })
                .to_string();
                let _ = sub.tx.send(frame);
            }
        }
    }
}

// ── WebSocket handler ──────────────────────────────────────────

/// Axum handler for `GET /cable`. Negotiates the
/// `actioncable-v1-json` subprotocol (Turbo's client requires the
/// echo) and hands off to the per-socket task.
pub async fn cable_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.protocols(["actioncable-v1-json"])
        .on_upgrade(handle_socket)
}

async fn handle_socket(socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();

    // Welcome frame — the Action Cable client waits for this before
    // it sends its first `subscribe`.
    if sender
        .send(Message::Text(
            json!({"type": "welcome"}).to_string().into(),
        ))
        .await
        .is_err()
    {
        return;
    }

    // Single outbound channel merges broadcasts + pings onto the
    // shared sender half. Cloning the tx into the ping task and
    // the registry lets each source push independently without
    // locking the socket writer.
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let tx_ptr = &tx as *const _ as usize;

    let ping_tx = tx.clone();
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
        // First tick fires immediately; skip it so we don't ping
        // before the welcome + confirm_subscription round-trip.
        interval.tick().await;
        loop {
            interval.tick().await;
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let frame = json!({"type": "ping", "message": ts}).to_string();
            if ping_tx.send(frame).is_err() {
                break;
            }
        }
    });

    let send_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if sender.send(Message::Text(frame.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = receiver.next().await {
        let Message::Text(text) = msg else { continue };
        let Ok(payload) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if payload.get("command").and_then(Value::as_str) != Some("subscribe") {
            continue;
        }
        let Some(identifier) = payload.get("identifier").and_then(Value::as_str) else {
            continue;
        };
        let Some(channel) = decode_channel(identifier) else {
            continue;
        };
        CABLE.subscribe(&channel, tx.clone(), identifier);
        let confirm = json!({
            "type": "confirm_subscription",
            "identifier": identifier,
        })
        .to_string();
        let _ = tx.send(confirm);
    }

    ping_task.abort();
    send_task.abort();
    CABLE.unsubscribe(tx_ptr);
}

/// Recover the channel name from Turbo's `signed_stream_name`.
/// The identifier is a JSON blob like
/// `{"channel":"Turbo::StreamsChannel","signed_stream_name":"<base64>--<digest>"}`;
/// the base64 segment holds a JSON-encoded channel name (e.g.
/// `"articles"`). If either decode fails we fall back to the raw
/// identifier so tests can subscribe by literal channel string.
fn decode_channel(identifier: &str) -> Option<String> {
    let id_json = serde_json::from_str::<Value>(identifier).ok()?;
    let signed = id_json
        .get("signed_stream_name")
        .and_then(Value::as_str)?;
    let base64_part = signed.split("--").next().unwrap_or("");
    let decoded_bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_part)
        .ok()?;
    let decoded = std::str::from_utf8(&decoded_bytes).ok()?;
    serde_json::from_str::<String>(decoded).ok()
}

// ── Broadcaster trait ──────────────────────────────────────────

/// Implemented on models with `broadcasts_to` declarations. The
/// emitter's `broadcasts_to` translation (Phase B) generates these
/// implementations; the runtime calls them from the generated
/// `save()` / `destroy()` methods at the end of a successful
/// persist.
///
/// Kept separate from `Model` so that models without any broadcast
/// hooks don't need a stub impl — the emitter only emits this for
/// models that declare `broadcasts_to`, and the save/destroy
/// codegen conditionally calls into it.
pub trait Broadcaster {
    fn after_save(&self);
    fn after_delete(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turbo_stream_html_wraps_content_in_template() {
        let got = turbo_stream_html("replace", "article_1", "<div>hi</div>");
        assert_eq!(
            got,
            r#"<turbo-stream action="replace" target="article_1"><template><div>hi</div></template></turbo-stream>"#
        );
    }

    #[test]
    fn turbo_stream_html_self_closes_when_empty() {
        let got = turbo_stream_html("remove", "article_1", "");
        assert_eq!(
            got,
            r#"<turbo-stream action="remove" target="article_1"></turbo-stream>"#
        );
    }

    #[test]
    fn dom_id_for_strips_trailing_s() {
        assert_eq!(dom_id_for("articles", 7), "article_7");
        assert_eq!(dom_id_for("comment", 3), "comment_3");
    }

    #[test]
    fn render_partial_falls_back_when_unregistered() {
        // Use a distinct type name so parallel tests don't collide.
        let got = render_partial("UnregisteredNoise", 99);
        assert_eq!(got, "<div>UnregisteredNoise #99</div>");
    }

    #[test]
    fn decode_channel_recovers_plain_base64_name() {
        // Construct a signed stream name for `"articles"`.
        let inner = serde_json::to_string("articles").unwrap();
        let b64 =
            base64::engine::general_purpose::STANDARD.encode(inner.as_bytes());
        let signed = format!("{}--unsigned", b64);
        let identifier = serde_json::json!({
            "channel": "Turbo::StreamsChannel",
            "signed_stream_name": signed,
        })
        .to_string();
        assert_eq!(decode_channel(&identifier).as_deref(), Some("articles"));
    }

    #[test]
    fn decode_channel_returns_none_on_bad_input() {
        assert!(decode_channel("not json").is_none());
        assert!(decode_channel(r#"{"no":"signed"}"#).is_none());
    }
}
