import { test, expect } from '@playwright/test'

// Flash shows exactly once, then is swept. Trigger a `redirect_to … notice:`
// → the redirect target renders the "successfully updated" notice → a later
// navigation must NOT show it again. The SWEEP (second assertion) is the
// regression that matters: a sticky flash re-renders the notice on every
// page (the bug that motivated moving the sweep into ActionDispatch::Flash).
//
// Why EDIT (not create): the suite runs `fullyParallel` against ONE shared
// server + DB, and `index.spec.js` pins the seeded set to exactly three
// articles with specific titles + comment counts. Creating an article would
// race that exact-count assertion; editing an existing one keeps the count,
// every title, and every comment count fixed — only the body (which no spec
// asserts) changes — so flash.spec stays isolated from its neighbours. The
// sweep is checked by reloading the article's own show page, not the index,
// to steer clear of index.spec entirely.
//
// Scoping (via E2E_SKIP in each target's README ## End-to-end block): runs
// only on cookie-backed, per-session targets (ruby, jruby, go, rust). The
// in-memory-flash targets (crystal, typescript) share ONE global flash slot,
// which races with the comment specs' `redirect_to … notice:` under
// `fullyParallel`; the remaining targets don't wire flash yet. As a target
// gains per-session flash, drop it from that skip list.
test('flash notice shows once then is swept', async ({ page }) => {
  await page.goto('/articles/1/edit')

  // Keep the title (index.spec pins it to this exact value); only the body
  // changes, which no spec asserts. An update with a valid body redirects
  // with the notice regardless of whether any field actually changed.
  await page.getByLabel('Title').fill('Getting Started with Rails')
  await page.getByLabel('Body').fill('Edited by the flash e2e spec to exercise the show-once sweep.')
  await page.getByRole('button', { name: 'Update Article' }).click()

  // Redirected to the article — the notice renders once.
  await expect(page.locator('#notice')).toHaveText('Article was successfully updated.')

  // Navigate again — the notice must be gone (swept), not sticky.
  await page.goto('/articles/1')
  await expect(page.locator('#notice')).toHaveCount(0)
})
