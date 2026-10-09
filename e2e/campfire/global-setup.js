import { chromium } from '@playwright/test'
import { mkdir } from 'node:fs/promises'
import { dirname } from 'node:path'
import { signIn } from './helpers.js'

// Signs in once per run and saves the session for every spec.
//
// The config resolves CAMPFIRE_BASE_URL and CAMPFIRE_AUTH_STATE (with
// defaults) before this runs, so both callers work: scripts/campfire-e2e
// sets them, the published archive's README run does not. `signIn`
// takes campfire's /first_run branch on the archive's fresh database.
export default async function globalSetup() {
  const baseURL = process.env.CAMPFIRE_BASE_URL
  const authState = process.env.CAMPFIRE_AUTH_STATE

  const browser = await chromium.launch()
  try {
    const context = await browser.newContext({ baseURL })
    const page = await context.newPage()
    await signIn(page)

    await mkdir(dirname(authState), { recursive: true })
    await context.storageState({ path: authState })
    await context.close()
  } finally {
    await browser.close()
  }
}
