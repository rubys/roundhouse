import { test, expect } from '@playwright/test'

test('a member signs in, opens their room, and signs out', async ({ browser }) => {
  const context = await browser.newContext({
    baseURL: process.env.CAMPFIRE_BASE_URL,
    storageState: { cookies: [], origins: [] },
  })
  const page = await context.newPage()
  try {
    await page.goto('/')
    await page.locator('#email_address').fill(process.env.CAMPFIRE_EMAIL || 'e2e@example.com')
    await page.locator('#password').fill(process.env.CAMPFIRE_PASSWORD || 'secret123456')
    await page.locator('button[name="log_in"]').click()
    await expect(page.locator('#user_sidebar')).toBeAttached({ timeout: 15_000 })

    const room = await page.goto('/rooms/1')
    expect(room?.status()).toBe(200)
    await page.goto('/users/me/profile')
    await page.getByRole('button', { name: 'Log out' }).click()
    await expect(page.locator('#email_address')).toBeVisible()
  } finally {
    await context.close()
  }
})
