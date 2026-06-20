// Hand-written roundhouse runtime primitive (no Ruby source).
// Action Cable WebSocket + Turbo Streams broadcaster (actioncable-v1-json).
// Raw Jetty 11 WebSocket servlet so the upgrade can negotiate the
// `actioncable-v1-json` subprotocol — Javalin's app.ws upgrades before its
// onConnect runs, too late to set the header ActionCable requires. Mirrors
// runtime/{go/v2,crystal,rust}/cable.

package roundhouse

import java.util.Base64
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.eclipse.jetty.servlet.ServletContextHandler
import org.eclipse.jetty.servlet.ServletHolder
import org.eclipse.jetty.websocket.api.Session
import org.eclipse.jetty.websocket.api.annotations.OnWebSocketClose
import org.eclipse.jetty.websocket.api.annotations.OnWebSocketConnect
import org.eclipse.jetty.websocket.api.annotations.OnWebSocketMessage
import org.eclipse.jetty.websocket.api.annotations.WebSocket
import org.eclipse.jetty.websocket.server.JettyWebSocketServlet
import org.eclipse.jetty.websocket.server.JettyWebSocketServletFactory
import org.eclipse.jetty.websocket.server.config.JettyWebSocketServletContainerInitializer
import org.json.JSONObject
import org.json.JSONTokener

object Cable {
    private data class Sub(val session: Session, val identifier: String)

    // channel name -> live subscriptions. The identifier (the raw subscribe
    // frame's `identifier` string) is echoed on every broadcast so Turbo
    // routes the frame to the right <turbo-cable-stream-source>.
    private val subscribers = ConcurrentHashMap<String, CopyOnWriteArrayList<Sub>>()
    private val sessions = CopyOnWriteArrayList<Session>()
    private val pinger = Executors.newSingleThreadScheduledExecutor { r ->
        Thread(r, "cable-ping").apply { isDaemon = true }
    }
    @Volatile private var pingerStarted = false

    // Mount the /cable servlet on Javalin's Jetty context. An exact servlet
    // mapping takes precedence over Javalin's greedy "/<path>" route.
    fun mount(handler: ServletContextHandler) {
        JettyWebSocketServletContainerInitializer.configure(handler, null)
        handler.addServlet(ServletHolder(CableServlet()), "/cable")
        synchronized(this) {
            if (!pingerStarted) {
                pingerStarted = true
                // ActionCable clients treat a ping gap (~6s) as a dead
                // connection and reconnect, so heartbeat every 3s.
                pinger.scheduleAtFixedRate({ pingAll() }, 3, 3, TimeUnit.SECONDS)
            }
        }
    }

    fun onConnect(session: Session) {
        sessions.add(session)
        safeSend(session, JSONObject().put("type", "welcome").toString())
    }

    fun onMessage(session: Session, message: String) {
        val frame = try { JSONObject(message) } catch (e: Exception) { return }
        if (frame.optString("command") != "subscribe") return
        val identifier = frame.optString("identifier")
        if (identifier.isEmpty()) return
        val channel = decodeChannel(identifier) ?: return
        subscribers.computeIfAbsent(channel) { CopyOnWriteArrayList() }.add(Sub(session, identifier))
        safeSend(
            session,
            JSONObject().put("type", "confirm_subscription").put("identifier", identifier).toString(),
        )
    }

    fun onClose(session: Session) {
        sessions.remove(session)
        for ((channel, subs) in subscribers) {
            subs.removeAll { it.session === session }
            if (subs.isEmpty()) subscribers.remove(channel, subs)
        }
    }

    // Fan `html` out to every subscriber of `channel`, wrapped in the Action
    // Cable message envelope Turbo expects. Called from Broadcasts on each
    // model after-commit hook.
    fun dispatch(channel: String, html: String) {
        val subs = subscribers[channel] ?: return
        for (sub in subs) {
            val msg = JSONObject()
                .put("type", "message")
                .put("identifier", sub.identifier)
                .put("message", html)
                .toString()
            safeSend(sub.session, msg)
        }
    }

    fun turboStreamHtml(action: String, target: String, content: String): String =
        if (content.isEmpty())
            "<turbo-stream action=\"$action\" target=\"$target\"></turbo-stream>"
        else
            "<turbo-stream action=\"$action\" target=\"$target\"><template>$content</template></turbo-stream>"

    private fun pingAll() {
        val now = System.currentTimeMillis() / 1000
        val frame = JSONObject().put("type", "ping").put("message", now).toString()
        for (session in sessions) safeSend(session, frame)
    }

    // Jetty's blocking sendString throws on concurrent sends to one socket
    // (a broadcast fiber racing the ping thread, or two creates fanning out
    // to a shared subscriber), so serialize per session.
    private fun safeSend(session: Session, msg: String) {
        if (!session.isOpen) return
        try {
            synchronized(session) { session.remote.sendString(msg) }
        } catch (e: Exception) {
            // socket closed between the snapshot and the write — onClose cleans up
        }
    }

    // Recover the channel name from Turbo's signed_stream_name. The identifier
    // is `{"channel":"Turbo::StreamsChannel","signed_stream_name":"<b64>--<digest>"}`;
    // the base64 prefix decodes to a JSON-encoded stream name (the same string
    // a broadcast's `stream` carries). Returns null on malformed input.
    private fun decodeChannel(identifier: String): String? = try {
        val signed = JSONObject(identifier).optString("signed_stream_name")
        val b64 = signed.substringBefore("--")
        val decoded = String(Base64.getDecoder().decode(b64))
        JSONTokener(decoded).nextValue() as? String
    } catch (e: Exception) {
        null
    }
}

// The Jetty WebSocket servlet for /cable. Its creator sets the accepted
// subprotocol during the upgrade handshake — the one thing Javalin's ws API
// can't do.
class CableServlet : JettyWebSocketServlet() {
    override fun configure(factory: JettyWebSocketServletFactory) {
        factory.setCreator { req, resp ->
            if (req.subProtocols.contains("actioncable-v1-json")) {
                resp.acceptedSubProtocol = "actioncable-v1-json"
            }
            CableEndpoint()
        }
    }
}

@WebSocket
class CableEndpoint {
    @OnWebSocketConnect
    fun onConnect(session: Session) = Cable.onConnect(session)

    @OnWebSocketMessage
    fun onMessage(session: Session, message: String) = Cable.onMessage(session, message)

    @OnWebSocketClose
    fun onClose(session: Session, statusCode: Int, reason: String?) = Cable.onClose(session)
}
