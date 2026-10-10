import { test, expect } from '@playwright/test'

// One route-level security contract, tested from outside the emitted app:
// a signed-out POST must not create a message, and the denied request must
// remain absent from the normal authenticated user's room.
test('a signed-out message POST is rejected without persisting a message', async ({ browser, page }) => {
  const body = `anonymous route probe ${Date.now()}`
  const anonymous = await browser.newContext()
  const anonymousPage = await anonymous.newPage()

  try {
    const response = await anonymousPage.request.post('/rooms/1/messages', {
      maxRedirects: 0,
      form: { 'message[body]': body },
      headers: {
        Origin: 'https://cross-site.invalid',
        'Sec-Fetch-Site': 'cross-site',
      },
    })

    // Rails can reject first at authentication (redirect) or at CSRF (422);
    // both are safe, but a success or server error is not an acceptable lane.
    expect([302, 401, 403, 422], 'the unauthenticated write is rejected').toContain(response.status())
    if (response.status() === 302) {
      expect(response.headers().location).toContain('/session/new')
    }
  } finally {
    await anonymous.close()
  }

  await page.goto('/rooms/1')
  await expect(
    page.locator('[id^="messages_"] .message').filter({ hasText: body }),
    'the rejected write did not persist a message',
  ).toHaveCount(0)
})
