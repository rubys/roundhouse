import { test, expect } from '@playwright/test'
import { signIn } from './helpers.js'

// THE AUTHORIZATION GUARD, END TO END.
//
// campfire prepends `RoomStreamsAreAuthorized` onto
// `Turbo::StreamsChannel` for one reason, stated in its own source: the
// stock channel "verifies only the signature on the stream name. That
// name carries no expiry and no binding to a user." So a signed stream
// name lifted off a page you can see is, without the guard, a
// subscription to a room you may not be in.
//
// This is the walk's "the stock channel refuses the room's :messages
// stream" check (`scripts/campfire-cable-drive.rb`), and it is the one
// check that must drive a RAW socket rather than campfire's client —
// the whole point is to send a frame campfire's own JavaScript would
// never send. `page.evaluate` gives us that from inside the origin, so
// the session cookie rides along exactly as it would for a real attempt.
//
// WHY THIS IS A MILESTONE PROBE AND NOT A LEDGER ENTRY. A failure here
// is a security divergence, not a missing feature: it means our emit
// subscribes a stolen stream name with no membership check. It was
// ledgered in docs/pipeline/runtime.md while the guard did not run at
// all; once the guard is in the path, a regression is a real defect and
// this spec is what catches it.
//
// A `confirm_subscription` here is the FAILURE we care about. A timeout
// is ambiguous — it could be a broken socket rather than an enforced
// guard — so the assertion distinguishes the two rather than treating
// "nothing arrived" as success.

test('the stock Turbo channel refuses a lifted stream name', async ({ page }) => {
  await signIn(page)
  await page.goto('/rooms/1')

  const source = page.locator('turbo-cable-stream-source').first()
  await expect(source).toBeAttached()
  const signed = await source.getAttribute('signed-stream-name')
  expect(signed, 'the room page minted a stream name').toBeTruthy()

  const outcome = await page.evaluate(async (signedStreamName) => {
    return await new Promise((resolve) => {
      const seen = []
      const socket = new WebSocket(`ws://${location.host}/cable`)
      const done = (result) => {
        try { socket.close() } catch { /* already closing */ }
        resolve({ ...result, seen })
      }
      const timer = setTimeout(() => done({ verdict: 'timeout' }), 15000)

      socket.onerror = () => { clearTimeout(timer); done({ verdict: 'socket-error' }) }
      socket.onmessage = (event) => {
        let message
        try { message = JSON.parse(event.data) } catch { return }
        if (message.type) seen.push(message.type)

        // Subscribe only after the welcome: a subscribe sent before the
        // connection is established is dropped, and the run then reads
        // as a timeout for a reason that has nothing to do with the guard.
        if (message.type === 'welcome') {
          socket.send(JSON.stringify({
            command: 'subscribe',
            identifier: JSON.stringify({
              channel: 'Turbo::StreamsChannel',
              signed_stream_name: signedStreamName,
            }),
          }))
        }
        if (message.type === 'reject_subscription') {
          clearTimeout(timer); done({ verdict: 'rejected' })
        }
        if (message.type === 'confirm_subscription') {
          clearTimeout(timer); done({ verdict: 'confirmed' })
        }
      }
    })
  }, signed)

  expect(
    outcome.verdict,
    `expected the stock channel to reject a lifted stream name; ` +
    `the socket reported ${outcome.verdict} (frames seen: ${outcome.seen.join(', ') || 'none'})`,
  ).toBe('rejected')
})
