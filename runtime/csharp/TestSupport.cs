using System;
using System.Collections.Generic;
using System.Linq;
using Xunit;

namespace Roundhouse;

// The hand-written half of the C# test harness; the app-specific half is
// the generated `TestSetup.cs` (RoundhouseTestSetup — schema DDL, fixture
// loaders, routes, controller factories). Port of
// `runtime/kotlin/test_support.kt` / `runtime/swift/test_support.swift`.
//
// Lifecycle: xUnit constructs a FRESH INSTANCE per test, so the
// constructor is the per-test setup — the analog of JUnit's @BeforeEach.
// A derived test class that ingested a `setup` method gets a generated
// constructor calling it, and C# runs the base constructor first, so the
// ordering (schema reset + fixtures, then the test's own setup) matches
// the other targets.
//
// The controller-test surface (Get/Post/Patch/Delete + AssertResponse /
// AssertRedirectedTo / AssertSelect) dispatches SYNCHRONOUSLY through the
// transpiled Router — the same path Server.Dispatch takes, minus HTTP and
// minus Kestrel. Assertion failures inside test bodies are plain
// `throw new Exception(...)` (what the `inline_assertions` lowerer emits),
// which xUnit records as a per-test failure; the harness's own checks use
// `Assert.Fail` for the same effect with a better message.
public class RoundhouseTestCase
{
    protected long __status = 200L;
    protected string __body = "";
    protected string __location = "";
    protected Flash __flash = new Flash();
    protected Session __session = new Session();

    public RoundhouseTestCase()
    {
        if (RoundhouseTestSetup.SchemaSql.Length > 0)
        {
            Db.SetupTestDb(RoundhouseTestSetup.SchemaSql);
            foreach (var loader in RoundhouseTestSetup.FixtureLoaders)
            {
                loader();
            }
        }
        ViewHelpers.ResetSlotsBang();
        __flash = new Flash();
        __session = new Session();
    }

    // ── controller-test dispatch ─────────────────────────────────

    public void Get(string path) =>
        PerformRequest("GET", path, new Dictionary<string, object?>());

    public void Post(string path, Dictionary<string, object?>? opts = null) =>
        PerformRequest("POST", path, RequestParams(opts));

    public void Patch(string path, Dictionary<string, object?>? opts = null) =>
        PerformRequest("PATCH", path, RequestParams(opts));

    public void Delete(string path, Dictionary<string, object?>? opts = null) =>
        PerformRequest("DELETE", path, RequestParams(opts));

    // `post path, params: { article: { … } }` lowers to a single options
    // hash; the request body is its "params" entry.
    private static Dictionary<string, object?> RequestParams(Dictionary<string, object?>? opts)
    {
        if (opts != null && opts.TryGetValue("params", out var p) &&
            p is Dictionary<string, object?> nested)
        {
            return nested;
        }
        return new Dictionary<string, object?>();
    }

    private void PerformRequest(string method, string path, Dictionary<string, object?> prms)
    {
        ViewHelpers.ResetSlotsBang();
        var match = Router.Match(method, path, RoundhouseTestSetup.Routes);
        if (match == null)
        {
            Assert.Fail($"no route for {method} {path}");
            return;
        }
        if (!RoundhouseTestSetup.Controllers.TryGetValue(match.Controller, out var factory))
        {
            Assert.Fail($"no controller registered for {match.Controller}");
            return;
        }

        var merged = new Dictionary<string, object?>(prms);
        foreach (var kv in match.PathParams)
        {
            merged[kv.Key] = kv.Value;
        }

        var controller = factory();
        controller.Params = merged;
        controller.RequestFormat = "html";
        controller.RequestMethod = method;
        controller.RequestPath = path;
        controller.Flash = __flash;
        controller.Session = __session;
        try
        {
            controller.ProcessAction(match.Action);
        }
        catch (RecordNotFound)
        {
            // Rails' rescue_from: a missing record is a 404, not a crash.
            __status = 404L;
            __body = "";
            __location = "";
            return;
        }
        __status = controller.Status;
        __body = controller.Body;
        __location = controller.Location ?? "";
        __flash = controller.Flash;
    }

    // ── HTTP response assertions ─────────────────────────────────

    private static readonly Dictionary<string, (long Lo, long Hi)> StatusRanges = new()
    {
        ["success"] = (200L, 299L),
        ["redirect"] = (300L, 399L),
        ["missing"] = (404L, 404L),
        ["not_found"] = (404L, 404L),
        ["error"] = (500L, 599L),
        ["ok"] = (200L, 200L),
        ["created"] = (201L, 201L),
        ["no_content"] = (204L, 204L),
        ["moved_permanently"] = (301L, 301L),
        ["found"] = (302L, 302L),
        ["see_other"] = (303L, 303L),
        ["bad_request"] = (400L, 400L),
        ["unauthorized"] = (401L, 401L),
        ["forbidden"] = (403L, 403L),
        ["unprocessable_entity"] = (422L, 422L),
        ["unprocessable_content"] = (422L, 422L),
        ["internal_server_error"] = (500L, 500L),
    };

    public void AssertResponse(string expected)
    {
        if (!StatusRanges.TryGetValue(expected, out var range))
        {
            Assert.Fail($"unknown response expectation {expected}");
            return;
        }
        if (__status < range.Lo || __status > range.Hi)
        {
            var preview = __body.Length > 200 ? __body.Substring(0, 200) : __body;
            Assert.Fail($"expected response {expected}, got status={__status} body={preview}");
        }
    }

    public void AssertRedirectedTo(string expectedPath)
    {
        if (__status < 300L || __status >= 400L)
        {
            Assert.Fail($"expected a redirect, got status={__status} location={__location}");
            return;
        }
        if (!__location.Contains(expectedPath))
        {
            Assert.Fail($"expected Location to contain {expectedPath}, got {__location}");
        }
    }

    // `AssertSelect` over the Dom primitive surface (below). Presence
    // check: the selector matches at least one node. The stub Dom is a
    // substring matcher, so this stays rough-but-effective for the
    // scaffold-blog HTML shapes; cardinality kwargs are best-effort
    // no-ops. A real engine tightens it without changing these sites.
    public void AssertSelect(string selector)
    {
        if (Dom.Select(Dom.Parse(__body), selector).Count == 0)
        {
            Assert.Fail($"expected body to match selector {selector}");
        }
    }

    // `content` is nullable: a nullable column read (`assert_select "h1",
    // article.title`) is `string?`, and Rails compares the element's text
    // against it — nil reads as the empty string, matching `nil.to_s`.
    public void AssertSelect(string selector, string? content)
    {
        var nodes = Dom.Select(Dom.Parse(__body), selector);
        if (nodes.Count == 0)
        {
            Assert.Fail($"expected body to match selector {selector}");
            return;
        }
        if (!nodes.Any(n => Dom.Text(n).Contains(content ?? "")))
        {
            Assert.Fail($"expected text {content} under selector {selector}");
        }
    }

    // `assert_select "h2", minimum: 1` — the cardinality kwargs arrive as
    // an options hash; presence is what the stub can honour.
    public void AssertSelect(string selector, Dictionary<string, object?> opts) =>
        AssertSelect(selector);

    // `assert_select "#articles" do … end` — the nested assertions run
    // against the same body (the stub has no scoping).
    public void AssertSelect(string selector, Action body)
    {
        AssertSelect(selector);
        body();
    }
}

// ── Dom primitive surface (the AssertSelect substrate) ─────────────
//
// The HTML-query contract AssertSelect lowers to, shared in shape with
// the Ruby/Kotlin/Swift/TS/Python/Rust/Elixir twins (cross-target
// contract in runtime/spinel/test/test_helper.rbs). Stub: the substring
// matcher dressed as a Dom — Select fabricates one synthetic node (the
// whole document) per fragment occurrence and Text returns it verbatim,
// so presence / minimum / content checks degrade to exactly the
// pre-contract behavior. The upgrade path is to swap these three methods
// for a real HTML parser (AngleSharp) — real nodes, real CSS selectors —
// touching only this class; the RoundhouseTestCase call sites stay put.
public static class Dom
{
    // Parse an HTML document. Stub: the document *is* its html string.
    public static string Parse(string html) => html;

    // Nodes matching `selector` within `root` (a document or node). Stub:
    // one synthetic node (the root's html) per substring-fragment
    // occurrence.
    public static List<string> Select(string root, string selector)
    {
        var fragment = FragmentFor(selector);
        var nodes = new List<string>();
        if (fragment.Length == 0) return nodes;
        var from = 0;
        while (true)
        {
            var i = root.IndexOf(fragment, from, StringComparison.Ordinal);
            if (i < 0) break;
            nodes.Add(root);
            from = i + fragment.Length;
        }
        return nodes;
    }

    // Concatenated descendant text of a node. Stub: the node verbatim.
    public static string Text(string node) => node;

    // Loose selector → substring fragment (the stub's rule, replaced by a
    // real CSS engine on upgrade): "#id" → id="id", ".cls" → cls", "tag"
    // → <tag. Compound selectors take the first chunk.
    private static string FragmentFor(string selector)
    {
        var first = selector.Split(' ').FirstOrDefault() ?? selector;
        if (first.StartsWith("#", StringComparison.Ordinal)) return "id=\"" + first.Substring(1) + "\"";
        if (first.StartsWith(".", StringComparison.Ordinal)) return first.Substring(1) + "\"";
        return "<" + first;
    }
}
