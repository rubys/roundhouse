import { test, expect } from '@playwright/test'

// article_3 is seeded with zero comments. Assertions are scoped to *our*
// uniquely-worded comment so this can run in parallel with the Turbo Stream
// test, which also posts comments on the same article.
const ARTICLE_PATH = '/articles/3'
const COMMENTER = 'Cable Bot'
const BODY = 'Action Cable broadcast smoke-test comment'

test('a new comment broadcasts live to other viewers via Action Cable', async ({ browser }) => {
  // Two independent contexts = two separate viewers of the same article.
  const observerCtx = await browser.newContext()
  const actorCtx = await browser.newContext()
  const observer = await observerCtx.newPage()
  const actor = await actorCtx.newPage()

  const observerRow = observer.locator('#comments > div').filter({ hasText: BODY })
  const actorRow = actor.locator('#comments > div').filter({ hasText: BODY })

  try {
    await observer.goto(ARTICLE_PATH)
    await actor.goto(ARTICLE_PATH)
    await expect(observerRow).toHaveCount(0)

    // Wait until the observer's Turbo Stream subscription is confirmed, so the
    // broadcast can't be missed by a not-yet-connected websocket.
    await expect(observer.locator('turbo-cable-stream-source')).toHaveAttribute('connected', '')

    // The observer must never navigate; this marker proves the row arrives over
    // the websocket rather than via a reload.
    await observer.evaluate(() => { window.__noNav = true })

    // The actor submits a comment.
    await actor.getByLabel('Commenter').fill(COMMENTER)
    await actor.getByLabel('Body').fill(BODY)
    await actor.getByRole('button', { name: 'Add Comment' }).click()

    // The observer receives it live, exactly once, without having navigated.
    await expect(observerRow).toHaveCount(1)
    await expect(observerRow).toBeVisible()
    expect(await observer.evaluate(() => window.__noNav)).toBe(true)

    // Cleanup: delete the comment from the actor page (accept the Turbo confirm).
    actor.on('dialog', dialog => dialog.accept())
    await actorRow.getByRole('button', { name: 'Delete' }).click()
    await expect(actorRow).toHaveCount(0)

    // The removal broadcasts too — the observer's row disappears, leaving no residue.
    await expect(observerRow).toHaveCount(0)
  } finally {
    await observerCtx.close()
    await actorCtx.close()
  }
})
