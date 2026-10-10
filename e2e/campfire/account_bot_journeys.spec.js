import { test, expect } from '@playwright/test'

// Account and bot journeys against the emitted Campfire. The harness
// boots one empty schema and creates a single administrator through
// `/first_run`; these specs must not rename that account or depend on
// rows another spec inserted. Unique names keep the shared SQLite file
// from colliding with a later run of the same file.
//
// Selectors and the bot JSON contract come from the pinned Campfire
// templates and controller tests (CAMPFIRE_SHA), not from YAML fixtures:
// `#invite_url` on Account Settings, `/join/:code` posting `user[name]`,
// `user[email_address]`, `user[password]`, the bot form's `user[name]`,
// and the readonly curl line `curl -d 'Hello!' <room bot messages url>`
// whose key is `{id}-{token}`. The bot route authenticates that key and
// takes the raw POST body as the message (Messages::ByBotsController).

const JOIN_CODE = /[A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4}/
const BOT_KEY = /(\d+)-([A-Za-z0-9]+)/

function stamp() {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

async function botRequest(page, method, path, body) {
  return page.evaluate(async ({ method, path, body }) => {
    const response = await fetch(path, {
      method,
      body,
      credentials: 'omit',
      headers: body === undefined ? {} : { 'content-type': 'text/plain; charset=utf-8' },
    })
    return {
      status: response.status,
      text: await response.text(),
      redirected: response.redirected,
      url: response.url,
    }
  }, { method, path, body })
}

async function createBotThroughAccountUi(page) {
  const id = stamp()
  const botName = `E2E Bot ${id}`

  await page.goto('/account/bots')
  const keysBefore = await page.getByLabel('curl command for posting messages').evaluateAll(inputs => inputs.map(input => input.value))

  await page.goto('/account/bots/new')
  await expect(page.locator('#user_name')).toBeVisible()
  await page.locator('#user_name').fill(botName)
  // Leave the webhook blank. Campfire delivers webhooks only for
  // mentions, and this journey must not call out to another host.
  await page.locator('#user_webhook_url').fill('')
  await page.getByRole('button', { name: 'Save changes' }).click()
  await expect(page).toHaveURL(/\/account\/bots\/?$/)

  const curls = page.getByLabel('curl command for posting messages')
  await expect(curls).toHaveCount(keysBefore.length + 1)
  const values = await curls.evaluateAll(inputs => inputs.map(input => input.value))
  const createdCurl = values.find(value => !keysBefore.includes(value))
  expect(createdCurl, 'the new bot card shows a message curl command').toBeTruthy()
  const postedTo = createdCurl.match(/curl -d 'Hello!' (\S+)/)
  expect(postedTo, `curl command ${JSON.stringify(createdCurl)}`).not.toBeNull()
  const messagesUrl = new URL(postedTo[1])
  const keyMatch = messagesUrl.pathname.match(/\/rooms\/(\d+)\/([^/]+)\/messages$/)
  expect(keyMatch, 'the curl URL is the room bot messages route').not.toBeNull()
  const roomId = keyMatch[1]
  const botKey = decodeURIComponent(keyMatch[2])
  expect(botKey).toMatch(BOT_KEY)
  expect(roomId, 'a new bot is a member of the open room created at first run').toBe('1')

  const botId = botKey.split('-')[0]
  return {
    id,
    botName,
    roomId,
    botKey,
    botId,
    // The edit control's visible content is an icon; its accessible name
    // is a `.for-screen-reader` span that Playwright's role locator does
    // not see. The href is the contract the template emits.
    card: page.locator('li').filter({ has: page.locator(`a[href="/account/bots/${botId}/edit"]`) }),
  }
}

async function signInAs(page, email, password) {
  await page.goto('/')
  await page.locator('#email_address').fill(email)
  await page.locator('#password').fill(password)
  await page.locator('button[name="log_in"]').click()
  await expect(page.locator('#user_sidebar')).toBeAttached({ timeout: 15_000 })
}

test('an invited member joins through the account invite URL and reaches the open room', async ({ browser }) => {
  const admin = await browser.newContext({
    baseURL: process.env.CAMPFIRE_BASE_URL,
    storageState: process.env.CAMPFIRE_AUTH_STATE,
  })
  const guest = await browser.newContext({
    baseURL: process.env.CAMPFIRE_BASE_URL,
    storageState: { cookies: [], origins: [] },
  })
  const adminPage = await admin.newPage()
  const guestPage = await guest.newPage()

  try {
    await adminPage.goto('/account/edit')
    await expect(adminPage).toHaveTitle(/Account settings/)

    const invite = await adminPage.locator('#invite_url').inputValue()
    const inviteUrl = new URL(invite)
    expect(inviteUrl.pathname, 'the invite field is Campfire\'s /join/:code URL').toMatch(/^\/join\//)
    const joinCode = inviteUrl.pathname.split('/').pop()
    expect(joinCode, 'the join code is the account\'s generated code').toMatch(JOIN_CODE)

    const id = stamp()
    const name = `E2E Invitee ${id}`
    const email = `e2e-invitee-${id}@example.com`
    const password = `invite-${id}`

    await guestPage.goto(inviteUrl.pathname)
    await expect(guestPage.locator('#user_name')).toBeVisible()
    await guestPage.locator('#user_name').fill(name)
    await guestPage.locator('#user_email_address').fill(email)
    await guestPage.locator('#user_password').fill(password)
    await guestPage.locator('button[type="submit"]').click()
    await expect(guestPage.locator('#user_sidebar')).toBeAttached({ timeout: 15_000 })
    await guestPage.goto('/rooms/1')
    await expect(guestPage.locator('.room--current, #composer').first()).toBeVisible()
    await expect(guestPage).not.toHaveURL(/\/session/)

    // A fresh context proves the account can sign in, not only that the
    // join POST left a session cookie behind.
    await guest.close()
    const returning = await browser.newContext({
      baseURL: process.env.CAMPFIRE_BASE_URL,
      storageState: { cookies: [], origins: [] },
    })
    const returningPage = await returning.newPage()
    try {
      await signInAs(returningPage, email, password)
      await returningPage.goto('/rooms/1')
      await expect(returningPage.locator('#composer')).toBeVisible()
      await expect(returningPage.locator('.room--current')).toContainText('All Talk')
    } finally {
      await returning.close()
    }
  } finally {
    await admin.close()
    await guest.close().catch(() => {})
  }
})

test('an administrator creates a bot, posts with its key, rotates the key, and removes the bot', async ({ page }) => {
  const { id, botName, roomId, botKey, card } = await createBotThroughAccountUi(page)
  const body = `E2E bot message ${id}`

  // Same contract as Messages::ByBotsControllerTest#create: the raw POST
  // body is the message, and the key in the path authenticates the bot.
  // Stay on the emitted origin; do not follow the curl line's host.
  // `text/plain` is what `curl -d` sends. A form body is parsed into
  // params and then discarded, so it cannot satisfy the raw-body route.
  const created = await botRequest(page, 'POST', `/rooms/${roomId}/${botKey}/messages`, body)
  const createdStatus = created.status
  const createdText = created.text
  expect(createdStatus, `bot create ${createdStatus} ${createdText}`).toBe(201)

  const listed = await botRequest(page, 'GET', `/rooms/${roomId}/${botKey}/messages`)
  expect(listed.status, `bot index ${listed.status} ${listed.text}`).toBe(200)
  const messages = JSON.parse(listed.text)
  const posted = messages.find(message => message?.body?.plain_text === body)
  expect(posted, 'the bot index returns the created message').toBeTruthy()
  expect(posted.creator.role).toBe('bot')
  expect(String(posted.creator.id)).toBe(botKey.split('-')[0])
  expect(String(posted.room.id)).toBe(roomId)
  // The name the administrator typed is what the JSON should report.
  // A param-key mismatch (`bot[name]` posted, `user` read) fails here
  // rather than being papered over by matching a blank name.
  expect(posted.creator.name, 'the bot keeps the name submitted in the account UI').toBe(botName)

  await page.goto(`/rooms/${roomId}`)
  await expect(page.locator('[id^="messages_"] .message').filter({ hasText: body })).toHaveCount(1)

  await page.goto('/account/bots')
  await card.locator(`a[href="/account/bots/${botKey.split('-')[0]}/edit"]`).click()
  await expect(page).toHaveURL(/\/account\/bots\/\d+\/edit$/)
  page.once('dialog', dialog => dialog.accept())
  await page.getByRole('button', { name: 'Generate a new key' }).click()
  await expect(page).toHaveURL(/\/account\/bots\/?$/)

  const rotatedCurl = await card.getByLabel('curl command for posting messages').inputValue()
  const rotatedTo = rotatedCurl.match(/curl -d 'Hello!' (\S+)/)
  expect(rotatedTo, 'rotating the key still shows a message command').not.toBeNull()
  const rotatedKey = new URL(rotatedTo[1]).pathname.match(/\/rooms\/\d+\/([^/]+)\/messages$/)[1]
  expect(rotatedKey, 'the displayed key changes when the bot key is reset').not.toBe(botKey)
  expect(rotatedKey).toMatch(BOT_KEY)

  const stale = await botRequest(page, 'POST', `/rooms/${roomId}/${botKey}/messages`, `stale ${id}`)
  expect(stale.redirected, 'the retired key is rejected rather than used').toBe(true)
  expect(stale.url).toContain('/session/new')

  const freshBody = `E2E rotated bot message ${id}`
  const fresh = await botRequest(page, 'POST', `/rooms/${roomId}/${rotatedKey}/messages`, freshBody)
  const freshStatus = fresh.status
  const freshText = fresh.text
  expect(freshStatus, `rotated key create ${freshStatus} ${freshText}`).toBe(201)
  const after = await botRequest(page, 'GET', `/rooms/${roomId}/${rotatedKey}/messages`)
  expect(after.status).toBe(200)
  const afterMessages = JSON.parse(after.text)
  expect(afterMessages.some(message => message?.body?.plain_text === freshBody)).toBe(true)
  expect(afterMessages.some(message => message?.body?.plain_text === `stale ${id}`)).toBe(false)

  await card.locator(`a[href="/account/bots/${botKey.split('-')[0]}/edit"]`).click()
  page.once('dialog', dialog => dialog.accept())
  await page.getByRole('button', { name: 'Delete this chat bot' }).click()
  await expect(page).toHaveURL(/\/account\/bots\/?$/)
  await expect(page.locator(`input[value*="${botKey}"], input[value*="${rotatedKey}"]`)).toHaveCount(0)

  const gone = await botRequest(page, 'POST', `/rooms/${roomId}/${rotatedKey}/messages`, `after delete ${id}`)
  expect(gone.redirected, 'a deactivated bot key no longer posts').toBe(true)
  expect(gone.url).toContain('/session/new')
})

test('an administrator rotates a bot key from the edit page and the old key stops authenticating', async ({ page }) => {
  const bot = await createBotThroughAccountUi(page)
  const { roomId, botKey, card } = bot

  await card.locator(`a[href="/account/bots/${botKey.split('-')[0]}/edit"]`).click()
  await expect(page).toHaveURL(/\/account\/bots\/\d+\/edit$/)
  page.once('dialog', dialog => dialog.accept())
  await page.getByRole('button', { name: 'Generate a new key' }).click()
  await expect(page).toHaveURL(/\/account\/bots\/?$/)

  const rotatedCurl = await card.getByLabel('curl command for posting messages').inputValue()
  const rotatedTo = rotatedCurl.match(/curl -d 'Hello!' (\S+)/)
  expect(rotatedTo, 'rotating the key still shows a message command').not.toBeNull()
  const rotatedKey = decodeURIComponent(new URL(rotatedTo[1]).pathname.match(/\/rooms\/\d+\/([^/]+)\/messages$/)[1])
  expect(rotatedKey, 'the displayed key changes when the bot key is reset').not.toBe(botKey)
  expect(rotatedKey).toMatch(BOT_KEY)

  const stale = await botRequest(page, 'POST', `/rooms/${roomId}/${botKey}/messages`, `stale ${stamp()}`)
  expect(stale.redirected, 'the retired key is rejected rather than used').toBe(true)
  expect(stale.url).toContain('/session/new')

  const listed = await botRequest(page, 'GET', `/rooms/${roomId}/${rotatedKey}/messages`)
  expect(listed.status, `the new key still lists messages (${listed.status})`).toBe(200)

  await card.locator(`a[href="/account/bots/${botKey.split('-')[0]}/edit"]`).click()
  page.once('dialog', dialog => dialog.accept())
  await page.getByRole('button', { name: 'Delete this chat bot' }).click()
  await expect(page).toHaveURL(/\/account\/bots\/?$/)
  await expect(page.locator(`input[value*="${botKey}"], input[value*="${rotatedKey}"]`)).toHaveCount(0)

  const gone = await botRequest(page, 'GET', `/rooms/${roomId}/${rotatedKey}/messages`)
  expect(gone.redirected, 'a deactivated bot key no longer reads the room').toBe(true)
  expect(gone.url).toContain('/session/new')
})
