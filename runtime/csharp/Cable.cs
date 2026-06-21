using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;

namespace Roundhouse;

// Action Cable WebSocket + Turbo Streams broadcaster (actioncable-v1-json).
// The .NET analog of runtime/kotlin/cable.kt and runtime/go/v2/cable.go:
// Server.cs accepts the /cable upgrade (negotiating the actioncable-v1-json
// subprotocol), HandleAsync runs the per-connection welcome → receive loop,
// and Dispatch fans a <turbo-stream> frame out to every subscriber of a
// channel. Broadcasts (the model after-commit sink) calls Dispatch.
public static class Cable
{
    private sealed class Sub
    {
        public WebSocket Socket = null!;
        public string Identifier = "";
    }

    // channel name -> live subscriptions. The identifier (the raw subscribe
    // frame's `identifier`) is echoed on every broadcast so Turbo routes the
    // frame to the right <turbo-cable-stream-source>. `Gate` guards both maps.
    private static readonly object Gate = new();
    private static readonly Dictionary<string, List<Sub>> Subscribers = new();
    private static readonly List<WebSocket> Sessions = new();
    private static readonly ConcurrentDictionary<WebSocket, SemaphoreSlim> SendLocks = new();
    private static Timer? _pinger;

    // ActionCable clients treat a ping gap (~6s) as a dead connection and
    // reconnect, so heartbeat every 3s (mirrors the other targets' cable).
    private static void EnsurePinger()
    {
        if (_pinger != null) return;
        lock (Gate)
        {
            _pinger ??= new Timer(
                _ => PingAll(), null, TimeSpan.FromSeconds(3), TimeSpan.FromSeconds(3));
        }
    }

    // Per-connection handler: welcome, then receive until the socket closes.
    public static async Task HandleAsync(WebSocket socket)
    {
        EnsurePinger();
        lock (Gate) Sessions.Add(socket);
        await SafeSend(socket, "{\"type\":\"welcome\"}");
        var buffer = new byte[8192];
        try
        {
            while (socket.State == WebSocketState.Open)
            {
                using var ms = new MemoryStream();
                WebSocketReceiveResult result;
                do
                {
                    result = await socket.ReceiveAsync(
                        new ArraySegment<byte>(buffer), CancellationToken.None);
                    if (result.MessageType == WebSocketMessageType.Close) return;
                    ms.Write(buffer, 0, result.Count);
                } while (!result.EndOfMessage);
                OnMessage(socket, Encoding.UTF8.GetString(ms.ToArray()));
            }
        }
        catch (Exception)
        {
            // socket dropped mid-receive — fall through to cleanup
        }
        finally
        {
            OnClose(socket);
        }
    }

    private static void OnMessage(WebSocket socket, string message)
    {
        JsonElement frame;
        try { frame = JsonDocument.Parse(message).RootElement; }
        catch (Exception) { return; }
        if (!frame.TryGetProperty("command", out var cmd) || cmd.GetString() != "subscribe") return;
        if (!frame.TryGetProperty("identifier", out var idEl)) return;
        var identifier = idEl.GetString() ?? "";
        if (identifier.Length == 0) return;
        var channel = DecodeChannel(identifier);
        if (channel == null) return;
        lock (Gate)
        {
            if (!Subscribers.TryGetValue(channel, out var subs))
            {
                subs = new List<Sub>();
                Subscribers[channel] = subs;
            }
            subs.Add(new Sub { Socket = socket, Identifier = identifier });
        }
        var confirm = JsonSerializer.Serialize(new Dictionary<string, object?>
        {
            ["type"] = "confirm_subscription",
            ["identifier"] = identifier,
        });
        _ = SafeSend(socket, confirm);
    }

    private static void OnClose(WebSocket socket)
    {
        lock (Gate)
        {
            Sessions.Remove(socket);
            var empty = new List<string>();
            foreach (var (channel, subs) in Subscribers)
            {
                subs.RemoveAll(s => s.Socket == socket);
                if (subs.Count == 0) empty.Add(channel);
            }
            foreach (var c in empty) Subscribers.Remove(c);
        }
        // Don't dispose the semaphore — an in-flight SafeSend may still hold it;
        // dropping the ref lets the GC reclaim it once that send completes.
        SendLocks.TryRemove(socket, out _);
    }

    // Fan `html` out to every subscriber of `channel`, wrapped in the Action
    // Cable message envelope Turbo expects. Called from Broadcasts on each
    // model after-commit hook (a synchronous request thread), so the per-socket
    // sends are fire-and-forget — SafeSend's semaphore keeps them ordered.
    public static void Dispatch(string channel, string html)
    {
        List<Sub> snapshot;
        lock (Gate)
        {
            if (!Subscribers.TryGetValue(channel, out var subs)) return;
            snapshot = new List<Sub>(subs);
        }
        foreach (var sub in snapshot)
        {
            var msg = JsonSerializer.Serialize(new Dictionary<string, object?>
            {
                ["type"] = "message",
                ["identifier"] = sub.Identifier,
                ["message"] = html,
            });
            _ = SafeSend(sub.Socket, msg);
        }
    }

    public static string TurboStreamHtml(string action, string target, string content) =>
        content.Length == 0
            ? $"<turbo-stream action=\"{action}\" target=\"{target}\"></turbo-stream>"
            : $"<turbo-stream action=\"{action}\" target=\"{target}\"><template>{content}</template></turbo-stream>";

    private static void PingAll()
    {
        List<WebSocket> snapshot;
        lock (Gate) snapshot = new List<WebSocket>(Sessions);
        var now = DateTimeOffset.UtcNow.ToUnixTimeSeconds();
        var frame = $"{{\"type\":\"ping\",\"message\":{now}}}";
        foreach (var session in snapshot) _ = SafeSend(session, frame);
    }

    // WebSocket.SendAsync isn't safe for concurrent sends on one socket (a
    // broadcast racing the ping timer), so serialize per socket.
    private static async Task SafeSend(WebSocket socket, string msg)
    {
        if (socket.State != WebSocketState.Open) return;
        var gate = SendLocks.GetOrAdd(socket, _ => new SemaphoreSlim(1, 1));
        try
        {
            await gate.WaitAsync();
        }
        catch (ObjectDisposedException)
        {
            return;
        }
        try
        {
            var bytes = Encoding.UTF8.GetBytes(msg);
            await socket.SendAsync(
                new ArraySegment<byte>(bytes), WebSocketMessageType.Text, true, CancellationToken.None);
        }
        catch (Exception)
        {
            // socket closed between the open check and the write — OnClose cleans up
        }
        finally
        {
            try { gate.Release(); } catch (ObjectDisposedException) { }
        }
    }

    // Recover the channel from Turbo's signed_stream_name. The identifier is
    // {"channel":"Turbo::StreamsChannel","signed_stream_name":"<b64>--<digest>"};
    // the base64 prefix decodes to a JSON-encoded stream name (the same string a
    // broadcast's `stream` carries). Returns null on malformed input.
    private static string? DecodeChannel(string identifier)
    {
        try
        {
            var root = JsonDocument.Parse(identifier).RootElement;
            if (!root.TryGetProperty("signed_stream_name", out var sEl)) return null;
            var signed = sEl.GetString() ?? "";
            var dash = signed.IndexOf("--", StringComparison.Ordinal);
            var b64 = dash >= 0 ? signed.Substring(0, dash) : signed;
            var decoded = Encoding.UTF8.GetString(Convert.FromBase64String(b64));
            var val = JsonDocument.Parse(decoded).RootElement;
            return val.ValueKind == JsonValueKind.String ? val.GetString() : null;
        }
        catch (Exception)
        {
            return null;
        }
    }
}
