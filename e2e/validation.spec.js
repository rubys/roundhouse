import { test, expect } from '@playwright/test'

test('new article form shows a validation error for a too-short body', async ({ page }) => {
  await page.goto('/articles/new')

  // Title only needs presence; body must be at least 10 chars. A short body
  // (9 chars) trips the length validation, so create re-renders :new (422).
  await page.getByLabel('Title').fill('Hi')
  await page.getByLabel('Body').fill('too short')

  await page.getByRole('button', { name: 'Create Article' }).click()

  // The error summary appears with the specific failure message.
  const errors = page.locator('#error_explanation')
  await expect(errors).toBeVisible()
  await expect(errors).toContainText('prohibited this article from being saved')
  await expect(errors).toContainText('Body is too short (minimum is 10 characters)')
})
