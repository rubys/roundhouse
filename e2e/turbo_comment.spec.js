import { test, expect } from '@playwright/test'

// article_3 ("Ruby2JS: Rails Everywhere") is seeded with zero comments. The
// assertions are scoped to *our* uniquely-worded comment (not the whole list)
// so this test can run in parallel with the Action Cable test, which also
// posts comments on the same article.
const ARTICLE_PATH = '/articles/3'
const COMMENTER = 'Playwright Bot'
const BODY = 'Turbo stream smoke-test comment'

test('adding a comment shows the new row via Turbo without a full reload', async ({ page }) => {
  await page.goto(ARTICLE_PATH)

  const ours = page.locator('#comments > div').filter({ hasText: BODY })
  await expect(ours).toHaveCount(0) // not present yet

  // Marker on window: a Turbo Drive visit preserves it; a full page reload wipes it.
  await page.evaluate(() => { window.__noFullReload = true })

  await page.getByLabel('Commenter').fill(COMMENTER)
  await page.getByLabel('Body').fill(BODY)
  await page.getByRole('button', { name: 'Add Comment' }).click()

  // Our new comment row appears exactly once (no redirect/broadcast duplicate).
  await expect(ours).toHaveCount(1)
  await expect(ours).toBeVisible()

  // Turbo handled the redirect as a Drive visit, not a full browser reload.
  expect(await page.evaluate(() => window.__noFullReload)).toBe(true)

  // Cleanup: delete the comment we added, accepting the Turbo confirm dialog.
  page.on('dialog', dialog => dialog.accept())
  await ours.getByRole('button', { name: 'Delete' }).click()
  await expect(ours).toHaveCount(0) // gone — no residue left behind
})
