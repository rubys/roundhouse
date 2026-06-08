import { test, expect } from '@playwright/test'

// The seeded database renders three articles, newest-first (prepend order).
// Each entry pins the DOM id, the title link text, and the comment-count
// label shown beside the title.
const expectedArticles = [
  { id: 'article_3', title: 'Ruby2JS: Rails Everywhere',      comments: '(0 comments)' },
  { id: 'article_2', title: 'Understanding MVC Architecture', comments: '(1 comment)' },
  { id: 'article_1', title: 'Getting Started with Rails',     comments: '(2 comments)' },
]

test('index lists the three seeded articles in order with correct comment counts', async ({ page }) => {
  await page.goto('/')

  // Exactly three articles render in the list.
  const rows = page.locator('#articles > div')
  await expect(rows).toHaveCount(expectedArticles.length)

  // Index positionally so the displayed order is asserted, not just presence.
  for (let i = 0; i < expectedArticles.length; i++) {
    const { id, title, comments } = expectedArticles[i]
    const row = rows.nth(i)
    await expect(row).toHaveAttribute('id', id)
    await expect(row.locator('h2 a')).toHaveText(title)
    await expect(row.locator(`#comments_count_${id}`)).toHaveText(comments)
  }
})
