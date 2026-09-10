# The WebMock double — handoff spec

> **Outcome (2026-09-10, landed in the commit that adds this file).**
> The double exists as written in §5–§6: `lower::webmock` rewrites every
> understood chain to `HttpStub.stub(...)`, the strict table is
> `runtime/spinel/http_stub.rb`, and the spinel-side seam is a REOPEN of
> `packages/net`'s `Net::HTTP` (`runtime/spinel/net_http.rb`) rather than
> a separate class, so `is_a?(Net::HTTPRedirection)` and the real transport
> both stay real. The ruby family gets a three-method delegate onto the gem.
>
> Measured against the same tree: spinel lane **+5**, not +22 —
> `webhook_test` 0/6 → 3/6, `user_bot_test` 1/4 → 2/4,
> `opengraph_location_test` 1/7 → 2/7. Ruby lane unchanged at 256/288, 42
> files green. Three corrections to the numbers below:
>
> * §4's "no `.with(...)` anywhere" is wrong by one: `webhook_test`'s
>   `payload` test writes `.with(body: hash_including(...))`. It is left
>   alone, as §5's rule says, and stays red until mocha's matchers land.
> * §2's +10/+4 for `opengraph_metadata_test` and `unfurl_links_controller_test`
>   counted only the FIRST failure. Behind the stub is `Nokogiri.HTML`,
>   a write-path gem façade on the strict targets, so those files' new
>   first failure is the parser, not the fetch.
> * The block form of `#request` (§3) is implemented but NOT shipped:
>   a yielding method has no dynamic-dispatch entry in spinel, and
>   `Webhook#http` comes back boxed, so a yielding `request` turned the
>   four delivery tests — and production webhook delivery — into
>   `NoMethodError`. It returns with matz/spinel#4420.
>
> Five spinel issues came out of the slice, all silent failures with
> reduced repros: #4416 (a reopened, scoped, yielding class method types
> its block parameter as the caller's class), #4417 (`k.new` on a nested
> class object answers nil — campfire's `request_class.new(url)`),
> #4418 (`StringIO.new.tap { read_body { } }.string` SIGSEGVs —
> campfire's `size_restricted_body`), #4419 (an undeclared keyword on a
> yielding method is dropped silently), #4420 (the two package gaps in §1).
> `Opengraph::Fetch` on spinel is gated on #4417 and #4418 regardless of
> the double. One unreduced finding: reading an element of an
> `Array[Hash[String, String]]` constant cost every test binary's
> analysis ~4 minutes, so the table stores headers as one String per stub.


Replace campfire's WebMock usage with a roundhouse-owned HTTP double that
sits ABOVE `Net::HTTP`, so the compiled lane never reaches a transport at
all. Self-contained slice: mostly new files, one settled design decision,
and a verification set of six test files that does not touch the lane's
headline number while in progress.

Measured 2026-09-10 against roundhouse `90ca1008`, campfire `94a48aa`,
spinel local master. Every number below is from a run, not an estimate.

---

## 1. The finding that forces the design

WebMock has exactly one interception point, so the obvious build is a stub
table inside the transport. **That build passes nothing here, because the
transport underneath is not the one campfire is written against.**

spinel bundles `packages/net` (`~/git/spinel/packages/net/net/http.rb`,
557 lines of Ruby, resolved implicitly — it needs no `-I` and emits no
"not available" warning). Two divergences from CRuby, both measured, both
silent:

```ruby
# probe2.rb — block form of #request
require "net/http"
require "uri"
url = URI.parse("http://127.0.0.1:8792/")
Net::HTTP.start(url.host, url.port, ipaddr: "127.0.0.1", use_ssl: false) do |http|
  http.request(Net::HTTP::Get.new(url)) do |response|
    puts "BLOCK code=#{response.code}"
  end
end
puts "done"
```

```
CRuby   : BLOCK code=200 / done
spinel  : done                    <- block never runs. No error, no warning.
```

```ruby
# probe3.rb — the ipaddr: pin
require "net/http"
require "uri"
begin
  r = Net::HTTP.start("nonexistent.invalid", 8792, ipaddr: "127.0.0.1", use_ssl: false) do |http|
    http.request(Net::HTTP::Get.new(URI.parse("http://nonexistent.invalid:8792/")))
  end
  puts "IPADDR HONORED code=#{r.code}"
rescue => e
  puts "IPADDR IGNORED: #{e.class}: #{e.message}"
end
```

```
CRuby   : IPADDR HONORED code=200
spinel  : IPADDR IGNORED: SystemCallError: ... @ connect - nonexistent.invalid
```

Serve both against a throwaway listener on 127.0.0.1:8792:

```ruby
require "socket"
s = TCPServer.new("127.0.0.1", 8792)
3.times do
  c = s.accept
  while (l = c.gets) && l.strip != ""; end
  c.write "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 2\r\n\r\nhi"
  c.close
end
```

Compile with `spinel probe2.rb -o probe2` (spinel refuses `-o /dev/null`).

`packages/net`'s `self.start` (line 270) takes no `ipaddr:`, and `#request`
(line 395) takes no block. `Opengraph::Fetch#request` is written on exactly
those two forms, so on the spinel binary today unfurl never yields — it
spins `MAX_REDIRECTS` times and raises `TooManyRedirectsError` — and the
`ipaddr:` pin that IS campfire's DNS-rebinding defence is not applied.

**Both are production bugs, not test-lane bugs, and both are upstream.**
File them regardless of what this slice does; they are 8-line repros. Per
`feedback_upstream_repro_first` and `feedback_check_upstream_head_before_filing`,
re-verify on current spinel master before filing, and cite
`/archive/campfire-suite/<stamp>/`, never `latest`.

The consequence for us: put the double above `Net::HTTP` and the transport
gaps are bypassed entirely. What we then owe is the RESPONSE side, which
campfire's own call sites bound tightly (§3).

---

## 2. Scope

### In

Six test files, all currently blocked on a webmock method as their FIRST
failure:

| file | spinel today | ruby ceiling | webmock yield |
|---|--:|--:|--:|
| `opengraph_metadata_test` | 0/10 | 10/10 | **+10** |
| `unfurl_links_controller_test` | 1/5 | 5/5 | **+4** |
| `webhook_test` | 0/6 | 5/6 | **+4** |
| `opengraph_location_test` | 1/7 | 7/7 | **+2** |
| `user_bot_test` | 1/4 | 4/4 | **+1** |
| `messages_controller_test` | 7/15 | 15/15 | **+1** |
| | | | **+22** |

**+22 tests, measured.** Not the ~35 the 40 call sites suggest — the rest
of those files' failures are mocha (`once`, `returns`, `with`, `raises`)
or unrelated (`id`, `dom_id`). Lane goes 116/288 -> 138/288.

`opengraph_fetch_test` (11 tests) is a seventh webmock-heavy file but is
BUILD-blocked at `test/opengraph_fetch_test.rb:142` on **matz/spinel#4415**
(`"image/png" != @fetch.fetch_content_type(url)` — a literal `!=` against a
nil-inferred call). Count it as unlocked-later, not as yield here.

### Out

- **mocha, both halves.** `stubs`/`returns` rows and the `expects`
  verification framework stay in this session; they land in
  `src/lower/mocha.rs`, which this slice must not touch.
- **`Net::HTTP::Persistent`.** `test/lib/web_push/persistent_request_test.rb`
  is not in the emitted `SPINEL_TESTS` (54 files, verified). The web push
  path is mocha-stubbed at `WebPush.payload_send`, not webmock-stubbed.
- **`WebMock.disable_net_connect! allow: [...]`** — 4 sites, 2 in the
  excluded persistent test and 2 in `fetch_test`'s DNS-rebinding tests,
  which pair it with `TCPSocket.expects(:open)`. Those are mocha's problem.
  Under the double there is no socket to allow; make the call a no-op and
  let the two tests stay red until `expects` lands.

---

## 3. The response surface, measured

Every `Net::HTTP` response method campfire reaches, across `app/` and
`lib/` (`version_headers.rb` excluded — that is an ActionDispatch
response, not this one):

| method | site | needs |
|---|---|---|
| `#code` | `webhook.rb:58` | String, `"200"` not `200` |
| `#body` | `webhook.rb:58,68` | String |
| `#[](name)` | `fetch.rb:20,30` | `"Content-Type"`, `"location"` — case-insensitive |
| `#content_type` | `webhook.rb:58,66`, `fetch.rb:73` | String or nil |
| `#content_length` | `fetch.rb:77` | Integer or nil (`.to_i` is applied) |
| `#read_body { \|chunk\| }` | `fetch.rb:57` | must YIELD, in chunks |
| `is_a?(Net::HTTPRedirection)` | `fetch.rb:29` | real class hierarchy |
| `is_a?(Net::HTTPOK)` | `fetch.rb:69` | real class hierarchy |

`is_a?` on the hierarchy is the reason the double cannot answer a bare
struct: status 302 must construct something that IS a
`Net::HTTPRedirection`, 200 an `Net::HTTPOK`. Mirror the subset
`packages/net` already declares (lines 83-98) rather than inventing names.

`#read_body` yielding is load-bearing: `size_restricted_body` bails when
the accumulated bytes cross `MAX_BODY_SIZE`, and three tests
(`content_length: 1.gigabyte`, `large_body_content`) exist only to exercise
that path. A `read_body` that returns the whole string without yielding
passes the happy path and silently breaks those three.

### Request surface the double must accept

- `Net::HTTP.start(host, port, ipaddr:, use_ssl:) { |http| ... }` — opengraph
- `Net::HTTP.new(host, port)` + `use_ssl=` / `open_timeout=` / `read_timeout=`,
  then `http.request(req)` with NO block — `webhook.rb:22-35`
- `Net::HTTP::Get.new(uri)`, `::Head.new(uri)`,
  `::Post.new(uri, "Content-Type" => "application/json")` with
  `request.body = payload`

---

## 4. The stub table

40 `stub_request` calls: 25 `:get`, 8 `:head`, 7 `:post`. **No `.with(...)`
request matchers anywhere** — matching is purely `(verb, url)`, which is
the single biggest simplification available and should be asserted, not
assumed, if the app is ever re-ingested.

`to_return` keys, counted: `status:` 37, `headers:` 34, `body:` 24,
and inside headers `content_type:` 26, `content_length:` 3, `location:` 4.
Both spellings occur — `content_type:` (symbol) and
`"Content-Type" =>` (string) — so normalise on the way in.

Follow the `runtime/ruby/resolv.rb` slot idiom exactly, and read its
comments before writing: **parallel constant Arrays, not a Hash and not a
module-level ivar** (the `Broadcasts::TRANSPORTS` pattern), each **seeded**
with a sentinel so spinel can infer an element type instead of dropping the
whole table behind unresolved-call gates. Install must REPLACE a matching
`(verb, url)` rather than append — `fetch_test` re-stubs the same URL
within one test.

---

## 5. The lowering

New module `src/lower/webmock.rs`, modelled on `src/lower/mocha.rs`:

```
WebMock.stub_request(:get, url).to_return(status: s, body: b, headers: h)
  ->  HttpStub.stub("GET", url, s, b, h)
```

Peel outside-in, as `mocha.rs::rewrite` does — `to_return` is the outermost
send. **A chain the pass does not fully understand is left alone**, keeping
its WebMock spelling and its loud failure. That rule is why the existing
mocha pass never silently drops a stub, and it is the failure mode that
matters: a stub that never took while the test reports green.

`WebMock.reset!` / `enable!` / `disable!` / `disable_net_connect!` lower to
table operations or no-ops.

---

## 6. The file swap — CRuby keeps real WebMock

A lowering may not branch on the target, so the rewrite is uniform and the
FILE is what differs, exactly as `resolv.rb` does today:

- `src/project.rs:1153` `fn ruby_runtime_files` — the swap
- `src/project.rs:1107` `const RESOLV_STUB_REOPEN` and its use at `:2145` —
  the precedent to copy

Ruby family gets an `HttpStub` whose `stub` delegates to
`WebMock.stub_request(...).to_return(...)`; strict targets get the table
plus the double. That preserves today's ruby lane (256/288) instead of
betting it on new code.

Wiring to update, and **this is the one file that collides with the mocha
work happening in parallel**:

- `src/project.rs:2748` `fn apply_test_gem_wiring` — the `TEST_GEMS` table
  still needs the webmock row for the ruby family
- `src/project.rs:2713` `fn patch_webmock_api_include`
- `src/project.rs:2658` `fn patch_mocha_lifecycle` — where the per-test
  clear calls are spliced. Read its comment about ordering before editing:
  inserting the requires before the `find`/`replace_range` pair shifted
  every offset and took every campfire test file down at once.

---

## 7. Traps

1. **Clear between tests or one test's stub answers for the rest of the
   file.** Shipping the resolv slot without a clear took
   `opengraph_location_test` from 7/7 to 6/7, and the test it broke does
   not stub DNS at all — it inherited the previous test's answer.
2. **Never guard the clear call on `defined?`.** On CRuby the constant can
   already exist for unrelated reasons; the guard then sails through to a
   `NoMethodError` in the setup of EVERY test. That is what took campfire's
   CRuby conformance from 255/288 to 34/288. Require the file instead —
   see `lower::mocha::stub_requires`.
3. **Read the generated C before theorising.** On this lane, four synthetic
   repros compiling cleanly has twice meant the theory was wrong, not the
   compiler right. `spinel -c` with `SP_COLLECT_ERRORS=1`, or the
   `kept at /tmp/spinel_out_*.c` path a failed build prints.
4. **Do not re-baseline the full sweep.** The mocha work is moving the same
   number from another session; one full sweep after both land, not two.

---

## 8. Verification

Emit once, then iterate against the retained tree:

```
scripts/campfire-suite --target spinel --jobs 8 --out /tmp/wm-emit \
  --filter 'opengraph|webhook_test|unfurl|bot_test|messages_controller' \
  --tally /tmp/wm-tally.txt --fail-log /tmp/wm-fail.txt
```

Re-run with `--reuse /tmp/wm-emit` to skip the transpile. One file, ~20 s:

```
make SPIN_TEST_FLAGS="$(spin flags|tail -1|sed 's/--require-gate //')" \
     SPINEL_TEST_FLAGS="-O 0 --no-inline-hot" build/test/<name>
```

Rank from the fail log, never the tally: the tally keeps only a file's
FIRST failure, which credits one wall with every unpassed test in the file.

**Before:** 10/89 on 11 files, 0 green.
**Target for this slice:** +22, and `opengraph_metadata_test`,
`unfurl_links_controller_test` green.

Gates before landing: `cargo test`, framework_tests ruby 9/9 and spinel
9/9, and the ruby lane re-verified at 256/288 / 42 files green — that last
one is the regression this design is shaped to avoid, so it is not
optional.

`spin lock` the emitted tree first: `spin flags` is not concurrency-safe
(matz/spinel#4393) and a `$(shell ...)` failure yields EMPTY flags, which
compiles successfully with no `-I`, no `--rbs` and no bcrypt.
