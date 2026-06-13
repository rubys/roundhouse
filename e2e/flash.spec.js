import { test, expect } from '@playwright/test'

// Flash shows exactly once, then is swept. Create an article → the
// redirect target renders the "successfully created" notice → a later
// navigation must NOT show it again. The SWEEP (second assertion) is the
// regression that matters: a sticky flash re-renders the notice on every
// page (the bug that motivated moving the sweep into ActionDispatch::Flash).
//
// Scoping (via E2E_SKIP in each target's README ## End-to-end block): this
// spec runs only on the cookie-backed, per-session targets (ruby, jruby).
// The in-memory-flash targets (crystal, typescript) share ONE global flash
// slot, which races with the comment specs' `redirect_to … notice:` under
// `fullyParallel`; the remaining targets don't wire flash yet. As a target
// gains per-session flash, drop it from that skip list.
test('flash notice shows once then is swept', async ({ page }) => {
  await page.goto('/articles/new')

  await page.getByLabel('Title').fill('Flash e2e article')
  await page.getByLabel('Body').fill('Exists only to exercise the flash sweep end to end.')
  await page.getByRole('button', { name: 'Create Article' }).click()

  // Redirected to the new article — the notice renders once.
  await expect(page.locator('#notice')).toHaveText('Article was successfully created.')

  // Navigate again — the notice must be gone (swept), not sticky.
  await page.goto('/articles')
  await expect(page.locator('#notice')).toHaveCount(0)
})
