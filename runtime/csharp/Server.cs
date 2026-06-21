using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Http;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Hosting;
using Microsoft.Extensions.Logging;

namespace Roundhouse;

// The Kestrel HTTP listener — the per-target server primitive (cf.
// runtime/kotlin/server.kt, runtime/crystal/server.cr). Parses the request,
// dispatches through the transpiled Router.match against the app's routes
// table, instantiates the matched controller, populates its request state
// (params/flash/session/format), runs process_action, and formats the
// response (redirect, html-with-layout, or json). The routes table, the
// controller factory map, and the layout function are passed in by the
// emitted Program.cs (they're app-specific).
public static class Server
{
    // Flash cookie name. Flash is cookie-backed and per-session (per browser),
    // so parallel clients never share a flash slot — the "show exactly once"
    // lifecycle lives in the transpiled Flash class; dispatch is just storage.
    private const string FlashCookie = "rh_flash";
    private static readonly string AssetsRoot = Path.GetFullPath("static/assets");

    public static void Start(
        int port,
        List<Route> routes,
        Dictionary<string, Func<ActionControllerBase>> controllers,
        Func<string, string?, string?, string> layout)
    {
        var builder = WebApplication.CreateBuilder();
        builder.Logging.ClearProviders();
        builder.WebHost.UseUrls($"http://0.0.0.0:{port}");
        var app = builder.Build();
        app.Run(ctx => Dispatch(ctx, routes, controllers, layout));
        Console.WriteLine($"Roundhouse C# server listening on http://0.0.0.0:{port}");
        app.Run();
    }

    private static async Task Dispatch(
        HttpContext ctx,
        List<Route> routes,
        Dictionary<string, Func<ActionControllerBase>> controllers,
        Func<string, string?, string?, string> layout)
    {
        ViewHelpers.resetSlotsBang();

        var reqMethod = ctx.Request.Method;
        var path = ctx.Request.Path.Value ?? "/";

        // Compiled assets (/assets/tailwind.css, …) — served before route
        // dispatch so the greedy app router doesn't 404 them.
        if (reqMethod == "GET" && path.StartsWith("/assets/"))
        {
            await ServeAsset(ctx, path.Substring("/assets/".Length));
            return;
        }

        IFormCollection? form = ctx.Request.HasFormContentType
            ? await ctx.Request.ReadFormAsync()
            : null;

        // Rails' `_method` override (button_to delete/patch forms POST).
        var method = reqMethod;
        if (method == "POST" && form != null && form.TryGetValue("_method", out var mo))
        {
            method = mo.ToString().ToUpperInvariant();
        }

        // A `.json` extension selects the JSON variant.
        var format = "html";
        if (path.EndsWith(".json"))
        {
            format = "json";
            path = path.Substring(0, path.Length - 5);
        }

        var match = Router.match(method, path, routes);
        var factory = match != null && controllers.TryGetValue(match.controller, out var f) ? f : null;
        if (match == null || factory == null)
        {
            ctx.Response.StatusCode = 404;
            await ctx.Response.WriteAsync("Not Found");
            return;
        }

        var prms = new Dictionary<string, object?>();
        foreach (var kv in match.pathParams) prms[kv.Key] = kv.Value;
        foreach (var kv in ctx.Request.Query)
        {
            if (kv.Value.Count > 0) SetParam(prms, kv.Key, kv.Value[0] ?? "");
        }
        if (form != null)
        {
            foreach (var kv in form)
            {
                if (kv.Value.Count > 0) SetParam(prms, kv.Key, kv.Value[0] ?? "");
            }
        }

        var controller = factory();
        controller.@params = prms;
        controller.requestFormat = format;
        controller.requestMethod = method;
        controller.requestPath = path;
        // Reload the flash carried from the previous request so views render it;
        // the constructor snapshots it as *_was so toPersisted can drop it
        // after one display.
        controller.flash = new Flash(ReadFlashCookie(ctx));
        controller.session = new Session();
        controller.processAction(match.action);

        WriteFlashCookie(ctx, controller.flash.toPersisted());

        var code = (int)controller.status;
        var location = controller.location;
        ctx.Response.StatusCode = code;
        if (location != null)
        {
            ctx.Response.Headers["Location"] = location;
        }
        else if (controller.requestFormat == "json")
        {
            ctx.Response.ContentType = "application/json";
            await ctx.Response.WriteAsync(controller.body);
        }
        else
        {
            ctx.Response.ContentType = "text/html; charset=utf-8";
            await ctx.Response.WriteAsync(layout(controller.body, controller.flash.notice, controller.flash.alert));
        }
    }

    // `article[title]=Foo` → a nested dictionary; a bare key → a scalar string.
    // Untyped params are held as `object?` so `<Resource>Params.from_raw`'s
    // `is`/`as` narrowing matches against real dictionary/string values.
    private static void SetParam(Dictionary<string, object?> prms, string key, string value)
    {
        var open = key.IndexOf('[');
        if (open >= 0 && key.EndsWith("]"))
        {
            var outer = key.Substring(0, open);
            var inner = key.Substring(open + 1, key.Length - open - 2);
            var dict = prms.GetValueOrDefault(outer) as Dictionary<string, object?>
                ?? new Dictionary<string, object?>();
            dict[inner] = value;
            prms[outer] = dict;
        }
        else
        {
            prms[key] = value;
        }
    }

    private static Dictionary<string, string> ReadFlashCookie(HttpContext ctx)
    {
        var outMap = new Dictionary<string, string>();
        var raw = ctx.Request.Cookies[FlashCookie];
        if (string.IsNullOrEmpty(raw)) return outMap;
        foreach (var pair in raw.Split('&'))
        {
            var idx = pair.IndexOf('=');
            if (idx <= 0) continue;
            var k = pair.Substring(0, idx);
            if (k != "notice" && k != "alert") continue;
            var v = Uri.UnescapeDataString(pair.Substring(idx + 1));
            if (v.Length > 0) outMap[k] = v;
        }
        return outMap;
    }

    private static void WriteFlashCookie(HttpContext ctx, Dictionary<string, string> persisted)
    {
        var parts = new List<string>();
        foreach (var k in new[] { "notice", "alert" })
        {
            if (persisted.TryGetValue(k, out var v))
            {
                parts.Add($"{k}={Uri.EscapeDataString(v)}");
            }
        }
        if (parts.Count == 0)
        {
            ctx.Response.Cookies.Delete(FlashCookie, new CookieOptions { Path = "/" });
            return;
        }
        ctx.Response.Cookies.Append(FlashCookie, string.Join("&", parts),
            new CookieOptions { Path = "/", HttpOnly = true });
    }

    // Serve a file from static/assets/, content-typed by extension; 404 when
    // missing (a fresh archive with no built assets still boots).
    private static async Task ServeAsset(HttpContext ctx, string rel)
    {
        var file = Path.GetFullPath(Path.Combine(AssetsRoot, rel));
        if (!file.StartsWith(AssetsRoot) || !File.Exists(file))
        {
            ctx.Response.StatusCode = 404;
            await ctx.Response.WriteAsync("Not Found");
            return;
        }
        ctx.Response.ContentType = Path.GetExtension(file).ToLowerInvariant() switch
        {
            ".css" => "text/css",
            ".js" or ".mjs" => "application/javascript",
            ".json" or ".map" => "application/json",
            ".svg" => "image/svg+xml",
            ".png" => "image/png",
            _ => "application/octet-stream",
        };
        await ctx.Response.Body.WriteAsync(await File.ReadAllBytesAsync(file));
    }
}
