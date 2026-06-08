import { test, expect } from '@playwright/test'

test('tailwind is compiled and applied', async ({ page }) => {
  await page.goto('/')

  // Stronger check: the compiled stylesheet actually serves (catches a broken
  // asset pipeline directly). Propshaft fingerprints the filename, so derive
  // the digested href from the page instead of hardcoding it.
  const hrefs = await page
    .locator('link[rel="stylesheet"]')
    .evaluateAll(links => links.map(l => l.getAttribute('href')))
  const tailwindHref = hrefs.find(h => /tailwind/.test(h)) ?? hrefs[0]
  expect(tailwindHref, 'expected a stylesheet <link> on the page').toBeTruthy()

  const res = await page.request.get(tailwindHref)
  expect(res.status()).toBe(200)
  expect(res.headers()['content-type']).toContain('css')

  // The "New article" button (an <a> styled via Tailwind utilities) is the probe.
  const button = page.getByRole('link', { name: 'New article' })
  const s = await button.evaluate(el => {
    const c = getComputedStyle(el)
    return { display: c.display, background: c.backgroundColor,
             radius: c.borderRadius, padding: c.paddingTop }
  })

  // Utilities applied, not browser defaults (an unstyled <a> would be
  // display:inline, transparent background, no radius, no padding):
  expect(s.display).toBe('block')                    // `block`
  expect(s.radius).toBe('6px')                       // `rounded-md`
  expect(s.padding).toBe('10px')                     // `py-2.5`
  expect(s.background).not.toBe('rgba(0, 0, 0, 0)')  // not transparent

  // The distinctive color token resolved (bg-blue-600):
  expect(s.background).toBe('oklch(0.546 0.245 262.881)')
})
