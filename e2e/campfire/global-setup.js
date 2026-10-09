import { chromium, expect } from '@playwright/test'
import { mkdir } from 'node:fs/promises'
import { dirname } from 'node:path'

export default async function globalSetup() {
  const baseURL = process.env.CAMPFIRE_BASE_URL || 'http://localhost:3000'
  const authState = process.env.CAMPFIRE_AUTH_STATE
  if (!authState) throw new Error('CAMPFIRE_AUTH_STATE must point to a per-run storage-state file')

  const browser = await chromium.launch()
  try {
    const context = await browser.newContext()
    const page = await context.newPage()
    await page.goto(baseURL)
    await page.locator('#email_address').fill(process.env.CAMPFIRE_EMAIL || 'e2e@example.com')
    await page.locator('#password').fill(process.env.CAMPFIRE_PASSWORD || 'secret123456')
    await page.locator('button[name="log_in"]').click()
    await expect(page.locator('#user_sidebar')).toBeAttached({ timeout: 15_000 })

    await mkdir(dirname(authState), { recursive: true })
    await context.storageState({ path: authState })
    await context.close()
  } finally {
    await browser.close()
  }
}
