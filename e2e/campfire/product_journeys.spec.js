import { test, expect } from '@playwright/test'

// These scenarios exercise Campfire through the browser against the emitted
// server, rather than calling Roundhouse internals or a Rails-side test app.
// Keep each journey on user-visible outcomes and use unique data because the
// emitted application's SQLite database lives for the whole suite run.

test('an administrator creates, renames, and deletes an open room', async ({ page }) => {
  const roomName = `E2E room ${Date.now()}`
  const renamedRoom = `${roomName} renamed`

  await page.goto('/rooms/opens/new')
  await page.locator('#room_name').fill(roomName)
  const form = page.locator('main form').first()
  expect(await form.evaluate(element => element.checkValidity()), 'the room form is valid before submit').toBe(true)
  const createRequest = page.waitForRequest(request => request.method() === 'POST')
  await page.getByRole('button', { name: 'Save' }).click()
  const postedTo = new URL((await createRequest).url())
  expect(postedTo.pathname, 'the room form posts to the open-room collection').toBe('/rooms/opens')
  await expect(page.locator('.room--current')).toContainText(roomName)

  await page.getByRole('link', { name: 'Settings for this room' }).click()
  await page.locator('#room_name').fill(renamedRoom)
  await page.getByRole('button', { name: 'Save' }).click()
  await expect(page.locator('.room--current')).toContainText(renamedRoom)

  page.once('dialog', dialog => dialog.accept())
  await page.getByRole('button', { name: `Delete ${renamedRoom}` }).click()
  await expect(page).toHaveURL(/\/rooms\/1$/)
  await expect(page.locator('#user_sidebar a').filter({ hasText: renamedRoom })).toHaveCount(0)
})

test('a member sends, boosts, edits, searches for, and deletes their message', async ({ page }) => {
  await page.goto('/rooms/1')

  const body = `E2E message ${Date.now()}`
  const editedBody = `${body} edited`
  const message = page.locator('[id^="messages_"] .message').filter({ hasText: body })

  const composer = page.locator('#composer lexxy-editor .lexxy-editor__content, #composer trix-editor#message_body').first()
  await composer.click()
  await page.keyboard.type(body)
  await page.locator('button[name="send"]').click()
  await expect(message).toHaveCount(1, { timeout: 15_000 })

  await message.locator('.message__options-btn').click()
  await message.locator('.message__boost-btn').click()
  const boostInput = message.getByRole('textbox', { name: 'Add a boost' })
  await boostInput.fill('Useful')
  await message.getByRole('button', { name: 'Submit' }).click()
  await expect(message.locator('.boost')).toContainText('Useful')

  await message.locator('.message__options-btn').click()
  await message.locator('.message__edit-btn').click()
  const editor = message.locator('.message__body-content--editing lexxy-editor#message_body')
  await expect(editor).toBeVisible()
  await editor.evaluate((element, value) => { element.value = value }, editedBody)
  expect(await editor.evaluate(element => element.value)).toContain(editedBody)
  const updateRequest = page.waitForRequest(request =>
    request.method() === 'POST' && /\/rooms\/1\/messages\/\d+$/.test(new URL(request.url()).pathname),
  )
  await message.getByRole('button', { name: 'Save changes' }).click()
  const update = await updateRequest
  expect(new URLSearchParams(update.postData()).get('message[body]')).toContain(editedBody)
  const updateResponse = await update.response()
  const updateStatus = updateResponse?.status() ?? 0
  // A redirect (Rails' HTML answer to an update) carries no readable body.
  const updateBody = updateStatus >= 400 ? ((await updateResponse?.text())?.slice(0, 200) ?? '') : ''
  expect(updateStatus, `message update ${update.url()} answered ${updateStatus}: ${updateBody}`).toBeLessThan(400)
  const editedMessage = page.locator('[id^="messages_"] .message').filter({ hasText: editedBody })
  await expect(editedMessage).toHaveCount(1)

  await page.goto('/searches')
  await page.getByRole('searchbox', { name: 'search' }).fill(editedBody)
  await page.getByRole('button', { name: 'Search', exact: true }).click()
  await expect(page.locator('#search-results .message').filter({ hasText: editedBody })).toHaveCount(1)

  await page.goto('/rooms/1')
  const currentMessage = page.locator('[id^="messages_"] .message').filter({ hasText: editedBody })
  await currentMessage.locator('.message__options-btn').click()
  await currentMessage.locator('.message__edit-btn').click()
  page.once('dialog', dialog => dialog.accept())
  await currentMessage.getByRole('button', { name: 'Delete message' }).click()
  await expect(page.locator('[id^="messages_"] .message').filter({ hasText: editedBody })).toHaveCount(0)
})

// No logout here: it would end the shared signed-in session every later
// spec starts from. auth_journey.spec.js covers logout in its own context.
test('a member updates their profile', async ({ page }) => {
  await page.goto('/users/me/profile')

  const updatedName = `E2E Profile ${Date.now()}`
  await page.locator('#user_name').fill(updatedName)
  await page.locator('#user_bio').fill('Profile changes are visible after saving.')
  // This UI also submits its optional password field. Keep the account's
  // known E2E credential intact so later tests remain independent.
  await page.locator('#user_password').fill(process.env.CAMPFIRE_PASSWORD || 'secret123456')
  await page.getByRole('button', { name: 'Save changes' }).click()
  await expect(page.locator('#user_name')).toHaveValue(updatedName)
  await expect(page.locator('#user_bio')).toHaveValue('Profile changes are visible after saving.')
})
