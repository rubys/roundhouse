import { test, expect } from '@playwright/test'
import { signIn } from './helpers.js'

// TYPING NOTIFICATIONS: one user types, another sees their name.
//
// The server half is campfire's `TypingNotificationsChannel#start`,
// which broadcasts `{ action: :start, user: current_user.slice(:id,
// :name) }`. Before `ActiveRecord::Base#slice` existed in the runtime,
// that call raised inside the channel action: the frame never went out,
// and the indicator stayed empty with nothing on the page to say why.
//
// TWO USERS, NOT TWO TABS. campfire's client drops a notification whose
// `user.id` is its own (`typing_notifications_controller.js#received`),
// so the cable spec's two-tabs-one-user shape would assert nothing. The
// second user joins through the account's own join link, the way a
// person invited to campfire does.
//
// Unique per run: the archive database persists across runs, and the
// join form refuses an email address that already has an account.
const STAMP = Date.now()
const TYPIST = `Typist ${STAMP}`

// Two sign-ins, a join, two room loads and a cable round trip; the 30s
// default is not a budget for that (see cable.spec.js).
test.setTimeout(90_000)

// `done` once the server has confirmed a `TypingNotificationsChannel`
// subscription on a socket that is still open. Action Cable confirms
// with `{"type":"confirm_subscription","identifier":"{\"channel\":…}"}`.
// Open sockets only: a page that navigates closes its socket, and the
// confirmation it had does not carry over to the next page.
function typingSubscribed(page) {
  const confirmed = new Set()
  page.on('websocket', ws => {
    ws.on('framereceived', ({ payload }) => {
      const frame = String(payload)
      if (frame.includes('confirm_subscription') && frame.includes('TypingNotificationsChannel')) confirmed.add(ws)
    })
    ws.on('close', () => confirmed.delete(ws))
  })
  return { get done() { return confirmed.size > 0 } }
}

test("a user typing in a room shows their name to the room's other members", async ({ browser }) => {
  const contextA = await browser.newContext()
  const contextB = await browser.newContext()
  const pageA = await contextA.newPage()
  const pageB = await contextB.newPage()

  try {
    // A is the first user, so an administrator, and the account page
    // shows them the join link.
    await signIn(pageA)
    await pageA.goto('/account/edit')
    const joinUrl = await pageA.locator('#invite_url').inputValue()
    expect(joinUrl, 'the account page shows a join link').toContain('/join/')

    // B joins. The join form is `users/new`, the same three fields the
    // first-run form has; landing is the signed-in layout's sidebar.
    await pageB.goto(new URL(joinUrl).pathname)
    await pageB.locator('#user_name').fill(TYPIST)
    await pageB.locator('#user_email_address').fill(`typist-${STAMP}@example.com`)
    await pageB.locator('#user_password').fill('secret123456')
    await pageB.locator('button[type="submit"]').click()
    await expect(pageB.locator('#user_sidebar')).toBeAttached({ timeout: 15_000 })

    // Typing only reaches A once both ends of the typing channel are
    // subscribed: B's to perform `start`, A's to receive it. The page
    // shows nothing for that subscription (the room stream's
    // `connected` attribute is a different one), so read the server's
    // confirmation off the socket. Watch before navigating, or the
    // frame can arrive first.
    const typingA = typingSubscribed(pageA)
    const typingB = typingSubscribed(pageB)

    // Room 1 is the first-run room, an open room every user belongs to.
    await pageA.goto('/rooms/1')
    await pageB.goto('/rooms/1')

    for (const [name, confirmed] of [['A', typingA], ['B', typingB]]) {
      await expect
        .poll(() => confirmed.done, { message: `tab ${name} is subscribed to typing notifications`, timeout: 20_000 })
        .toBe(true)
    }

    // Typed keys, not `fill()`: the editor is a custom element, and the
    // `lexxy:change` event that drives `typing-notifications#start`
    // fires from real input (see cable.spec.js).
    await pageB
      .locator('lexxy-editor#message_body .lexxy-editor__content, trix-editor#message_body')
      .first()
      .click()
    await pageB.keyboard.type('hello')

    const indicator = pageA.locator('[data-typing-notifications-target="indicator"]')
    await expect(
      pageA.locator('[data-typing-notifications-target="author"]'),
      "A sees B's name in the typing indicator",
    ).toHaveText(TYPIST, { timeout: 15_000 })
    await expect(indicator, 'the indicator is shown').toHaveClass(/typing-indicator--active/)
  } finally {
    await contextA.close()
    await contextB.close()
  }
})
