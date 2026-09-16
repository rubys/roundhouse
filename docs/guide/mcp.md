# The agent — `roundhouse mcp`

`roundhouse mcp` serves the analyzer to a coding agent over the Model
Context Protocol, on stdio. Claude Code, Cursor, or anything else that
speaks MCP can then ask a Rails app the questions grep cannot answer:
*what type is this expression, can it be nil, which filters run before
this action and in what order, which partials does this view reach,
which queries are N+1, which gems does the analyzer not understand,
and what in this app will not survive being compiled to Go?*

Every answer is derived statically from the source on disk. Nothing is
booted, and each call re-reads the tree, so an agent that edits a file
and asks again gets an answer about the edited file. Ingest runs in
survey mode — an unsupported construct records a gap instead of
aborting — so one exotic node somewhere does not take the whole server
down.

## Setup

The server takes the app's root as its one argument (default:
`$ROUNDHOUSE_APP_ROOT`, else the working directory), and needs the
[`roundhouse` binary](install.md) on `PATH`.

**Claude Code**, from inside the app:

```sh
claude mcp add roundhouse -- roundhouse mcp .
```

or, checked in for the whole team, a `.mcp.json` at the app's root:

```json
{
  "mcpServers": {
    "roundhouse": { "command": "roundhouse", "args": ["mcp", "."] }
  }
}
```

**Cursor** reads the same shape from `.cursor/mcp.json`. Other clients
differ only in where the file lives; the command is always
`roundhouse mcp <app-root>` over stdio.

`roundhouse --version` and the server's `serverInfo` name the same
build, so a client's MCP log tells you which snapshot answered.

## The tools

| Tool | Answers |
|---|---|
| `type_at` | The inferred type of the expression at `path:line[:column]`, whether it can be nil, and the node kind. Works on code that does not parse. |
| `can_be_nil` | Just the nil question, for the same position. |
| `references` | Every read and write of the local or instance variable at a position — a local by its exact binding, an ivar across the class and the views it feeds. |
| `diagnostics` | The app's analysis problems, filterable by `path`, `severity` floor, diagnostic `code`, and `limit`. Ends with the `missing_preload` coverage triple — chains checked, findings, unverifiable — so a clean N+1 report states its denominator. |
| `traceroute` | The full static request flow for `Controller#action` or `[VERB ]/path`: the route match, every before/around/after filter with its defining class or concern, its `only:`/`except:`/`if:`/`unless:` gating (decided against the route's verb where the guard allows it), what it assigns and its DB effects, the action, the view with its partials, the layout — plus N+1 findings on the hop that contains them, and a coverage footer naming what could not be resolved and why. |
| `trace_targets` | Every entry point `traceroute` can trace — each concrete route and each unrouted action that renders a template — optionally narrowed to controllers whose name contains a string. A trace query is picked from this list, not guessed. |
| `related_files` | Files connected to a source file by analyzed edges, not naming convention: the views a controller's actions feed, the partials a view renders and the views that render it, a concern's includers, and the controller/model twin. |
| `gems` | The gem census from `Gemfile.lock` — framework, stdlib, modeled, infrastructure, unknown — unknown first, with how many current diagnostics each accounts for. |
| `wont_lower` | Which constructs in the app will not compile to a given transpile target (`rust`, `go`, `typescript`, …). |

[`check.md`](check.md) explains what each diagnostic code means and
which are the app's problem versus roundhouse's; the `diagnostics`
tool's output uses the same codes and the same gap attribution.

## What to expect

An agent with these tools reaches for them unprompted on the questions
they fit. On the maintainer-style questions the project uses to
measure this against Campfire — *which filters run before this action
and in what order*, *which views render this partial*, *where is this
relation iterated without a preload* — the tool arm answers from the
analyzer's edges where the grep arm reconstructs them by reading
files, and on the filter-order question that is the difference between
twelve hops in the right order in a few seconds and eleven of twelve
after three minutes of reading. On questions grep answers well — the
type of one obvious local — the tool adds a round trip and no
information; that is fine, and an agent that skips it there is
behaving correctly.

The trace and the related-files answers are the ones no other Rails
tool provides statically, and the ones an agent is most likely to
second-guess with a grep. `render @message` reaches a partial by
convention, and grep will not see the edge; the tool's answer is the
one to trust.

The harness that produces those numbers is `scripts/mcp-ab` in the
repository: the same questions, with and without the server, scored
against a source-derived key. It doubles as a bug-finder for the
analyzer, and it is what a report of "the tool told my agent something
wrong" will be reproduced with — include the question and the answer.
