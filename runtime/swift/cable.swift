import Foundation
import HummingbirdWebSocket
import NIOConcurrencyHelpers

enum Cable {
    struct Sub {
        let connId: UUID
        let identifier: String
        let cont: AsyncStream<String>.Continuation
    }

    private static let lock = NIOLock()
    // channel name -> live subscriptions. The identifier (the raw
    // subscribe frame's `identifier` string) is echoed on every
    // broadcast so Turbo routes the frame to the right
    // <turbo-cable-stream-source>.
    private static var subscribers: [String: [Sub]] = [:]

    // The /cable connection handler: welcome -> (subscribe ->
    // confirm_subscription)* with a writer task draining the broadcast
    // stream and a ping task heartbeating.
    static func handle(
        _ inbound: WebSocketInboundStream,
        _ outbound: WebSocketOutboundWriter
    ) async {
        let connId = UUID()
        let (stream, cont) = AsyncStream.makeStream(of: String.self)
        cont.yield(encode(["type": "welcome"]))
        let writer = Task {
            for await msg in stream {
                do {
                    try await outbound.write(.text(msg))
                } catch {
                    break
                }
            }
        }
        let pinger = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 3_000_000_000)
                cont.yield(encode([
                    "type": "ping",
                    "message": Int(Date().timeIntervalSince1970),
                ]))
            }
        }
        do {
            for try await message in inbound.messages(maxSize: 1 << 20) {
                if case .text(let text) = message {
                    onMessage(connId, cont, text)
                }
            }
        } catch {
            // socket error — fall through to cleanup
        }
        pinger.cancel()
        cont.finish()
        lock.withLock {
            for (channel, subs) in subscribers {
                let kept = subs.filter { $0.connId != connId }
                if kept.isEmpty {
                    subscribers.removeValue(forKey: channel)
                } else {
                    subscribers[channel] = kept
                }
            }
        }
        _ = await writer.value
    }

    private static func onMessage(
        _ connId: UUID,
        _ cont: AsyncStream<String>.Continuation,
        _ text: String
    ) {
        guard let data = text.data(using: .utf8),
              let frame = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              frame["command"] as? String == "subscribe",
              let identifier = frame["identifier"] as? String,
              let channel = decodeChannel(identifier)
        else { return }
        lock.withLock {
            subscribers[channel, default: []].append(
                Sub(connId: connId, identifier: identifier, cont: cont)
            )
        }
        cont.yield(encode(["type": "confirm_subscription", "identifier": identifier]))
    }

    // Fan `html` out to every subscriber of `channel`, wrapped in the
    // Action Cable message envelope Turbo expects. Called from
    // Broadcasts on each model after-commit hook (a Db pool thread —
    // the continuation yield is the thread-safe bridge).
    static func dispatch(_ channel: String, _ html: String) {
        let subs = lock.withLock { subscribers[channel] ?? [] }
        for sub in subs {
            sub.cont.yield(encode([
                "type": "message",
                "identifier": sub.identifier,
                "message": html,
            ]))
        }
    }

    static func turboStreamHtml(_ action: String, _ target: String, _ content: String) -> String {
        if content.isEmpty {
            return "<turbo-stream action=\"\(action)\" target=\"\(target)\"></turbo-stream>"
        }
        return "<turbo-stream action=\"\(action)\" target=\"\(target)\"><template>\(content)</template></turbo-stream>"
    }

    // Recover the channel name from Turbo's signed_stream_name. The
    // identifier is `{"channel":"Turbo::StreamsChannel",
    // "signed_stream_name":"<b64>--<digest>"}`; the base64 prefix
    // decodes to a JSON-encoded stream name (the same string a
    // broadcast's `stream` carries).
    private static func decodeChannel(_ identifier: String) -> String? {
        guard let idData = identifier.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: idData) as? [String: Any],
              let signed = obj["signed_stream_name"] as? String
        else { return nil }
        let b64 = signed.components(separatedBy: "--").first ?? signed
        guard let decoded = Data(base64Encoded: b64),
              let name = try? JSONSerialization.jsonObject(
                  with: decoded,
                  options: [.fragmentsAllowed]
              ) as? String
        else { return nil }
        return name
    }

    private static func encode(_ obj: [String: Any]) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: obj),
              let s = String(data: data, encoding: .utf8)
        else { return "{}" }
        return s
    }
}
