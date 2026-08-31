import { test, expect } from '@playwright/test'
import { signIn, watchForFailures, triage, report } from './helpers.js'

// THE MILESTONE, IN A BROWSER: two tabs, one room, one live message.
//
// This is the browser port of `scripts/campfire-cable-drive.rb` — the
// cable walk. That script drives two raw WebSockets with a hand-built
// subscribe frame and reads the turbo-stream payload off the wire; this
// drives campfire's OWN client, and asserts the outcome the wire was
// carrying. Neither replaces the other, and the split is deliberate:
//
//   - The walk can assert things a browser cannot see — that the frame
//     echoes the subscription identifier byte for byte, that the payload
//     is an `append` rather than a `replace`. It also fails with a much
//     better diagnosis, because it names the frame that was wrong.
//   - This spec can assert the thing the walk cannot: that campfire's
//     own JavaScript — Turbo, Stimulus, the cable consumer, ninety-five
//     ES modules loaded from `static/assets/` — actually boots and does
//     the right thing with what arrives. A walk that is green while
//     Turbo never loaded is a walk that proves the server half only.
//
// WHY THE ARCHIVE SHIPS THIS ONE AND NOT THE WALK. The walk is Ruby, and
// needs `websocket-driver` and `net/http`. A downloader of the published
// archive has spinel, a C compiler and Node — no Ruby, and no gems. The
// browser is the only driver the archive can assume.
//
// Mapping onto the walk's checks, so a change to one can be reflected in
// the other:
//
//   walk check                                   | here
//   ---------------------------------------------+-------------------------
//   the account has a session cookie             | signIn's landing assertion
//   GET /rooms/1                                 | goto + response check
//   the room page carries a stream source        | the locator below
//   the page names the app's own channel         | the channel attribute
//   both connections are welcomed                | [connected] on both tabs
//   both subscriptions are confirmed             | [connected] on both tabs
//   POST /rooms/1/messages                       | submitting the composer
//   connection A receives the broadcast          | the message in tab A
//   connection B receives the broadcast          | the message in tab B
//   it is an append to the room's message list   | asserted INSIDE #messages_room_1
//   it carries the message that was posted       | the body text
//   the frame echoes the subscription identifier | NOT OBSERVABLE — walk only
//   the stock channel refuses the stream         | cable_guard.spec.js
//
// `--unsigned` is expected in the stream name: the emitted tree mints
// unsigned stream names on purpose (the guard, not the signature, is what
// authorizes a subscribe — see cable_guard.spec.js).

const BODY = 'hello from the cable spec'

// Playwright's default is 30s for a whole test, and this one does more
// than any other in the suite: two sign-ins (the first of which completes
// campfire's first-run form and creates the account, the user and the
// room), two room loads of ~130 subresources each, two subscription
// waits, and a Trix interaction. The default is not a meaningful budget
// for that, and blowing it reports as "browserContext.close: Test ended"
// pointing at the cleanup line — which says nothing about what was slow.
test.setTimeout(90_000)

// FIXME, NOT SKIP, and the distinction is the point: this spec is
// CORRECT and the app is wrong. One defect remains, ledgered in
// docs/pipeline/runtime.md § An HTML message body renders as nothing:
// `sanitize_allowing` is a raising façade on this target, campfire's
// `rescue Exception` turns the raise into "", and Trix always submits
// HTML — so every composer-posted message body renders empty, on the
// page and in the frame alike. (The spec's other original blocker, the
// ~30s second-client stall, closed 2026-08-31 — it was a truncated
// /account/logo response, not a serializing server.)
//
// `test.fixme` runs nothing and Playwright reports it as
// expected-broken; when the sanitizer entry closes, delete this marker
// and the suite gains its fourth test — and raise the scripts/smoke
// campfire floor 3 -> 4 in the same commit. Do NOT convert this to
// `test.skip` — skip says "not applicable here", which is false.
test.fixme('a message posted in one tab arrives live in another', async ({ browser }) => {
  // Two independent contexts, not two pages in one — separate cookie
  // jars and separate cable connections, which is what "a second
  // connection" means in the milestone. Two pages in one context can
  // share a connection and would prove less.
  const contextA = await browser.newContext()
  const contextB = await browser.newContext()
  const pageA = await contextA.newPage()
  const pageB = await contextB.newPage()

  const findingsA = watchForFailures(pageA)

  try {
    // Tab A signs in first: on a fresh archive database this is the
    // /first_run POST that creates the account, the user and room 1, so
    // it must complete before B tries to sign in.
    await signIn(pageA)
    await signIn(pageB)

    const responseA = await pageA.goto('/rooms/1')
    expect(responseA?.status(), 'GET /rooms/1').toBe(200)
    await pageB.goto('/rooms/1')

    // The room page carries a stream source, and it names the app's OWN
    // channel. campfire routes the subscription away from the stock
    // Turbo::StreamsChannel deliberately; if this attribute ever reads
    // `Turbo::StreamsChannel`, the guard in cable_guard.spec.js is being
    // bypassed rather than enforced.
    const source = pageA.locator('turbo-cable-stream-source').first()
    await expect(source).toBeAttached()
    await expect(source).toHaveAttribute('channel', 'RoomMessagesChannel')

    // `connected` is set by Turbo's own cable consumer once the socket
    // is open AND the subscription is confirmed — the browser-visible
    // equivalent of the walk's "welcomed" plus "confirmed" pair.
    for (const [name, page] of [['A', pageA], ['B', pageB]]) {
      await expect(
        page.locator('turbo-cable-stream-source[connected]').first(),
        `tab ${name} subscribed to the room stream`,
      ).toBeAttached({ timeout: 20_000 })
    }

    // Post through the real composer. Trix is a custom element, so it
    // takes focus + typed keys — `fill()` targets the hidden input and
    // would submit an empty body while looking like it worked.
    await pageA.locator('trix-editor#message_body').click()
    await pageA.keyboard.type(BODY)
    await pageA.locator('button[name="send"]').click()

    // The assertion is scoped INSIDE the room's message list, which is
    // what makes it the "append to the room's message list" check rather
    // than "the text appears somewhere on the page".
    await expect(
      pageB.locator('#messages_room_1'),
      'tab B received the broadcast',
    ).toContainText(BODY, { timeout: 20_000 })
    await expect(
      pageA.locator('#messages_room_1'),
      'tab A received the broadcast',
    ).toContainText(BODY, { timeout: 20_000 })

    // REPORTED, NOT ASSERTED. A live message that arrived over a page
    // whose modules 404'd is worth knowing about, so the findings are
    // printed — but asserting on them here would make the MILESTONE spec
    // fail for a reason assets.spec.js already owns, and the walk's own
    // milestone/ledger split exists to stop exactly that. One spec, one
    // claim: this one says the message arrived.
    const { open } = triage([
      ...findingsA.responses, ...findingsA.failed,
      ...findingsA.console, ...findingsA.errors,
    ])
    if (open.length) {
      console.log(`note: the room page reported failures (see assets.spec.js):\n${report(findingsA)}`)
    }
  } finally {
    await contextA.close()
    await contextB.close()
  }
})
