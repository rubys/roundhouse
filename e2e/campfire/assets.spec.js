import { test, expect } from '@playwright/test'
import { signIn, watchForFailures, triage, report } from './helpers.js'

// The first campfire browser spec, and deliberately the least clever
// one: it asserts that nothing 404s and nothing throws. No layout, no
// behaviour, no opinion about what the page should contain.
//
// WHY THIS SHAPE. The room page pins ninety-five ES modules and links
// twenty-six stylesheets. Until `18e1601f` the build produced NONE of
// them — `make assets` in a campfire emit died on the blog's
// `hello_controller.js`, `app/assets/stylesheets/` was never walked into
// the tree, and eighty SVG icons were dropped because an SVG is text and
// the binary-asset collector only carries what is not valid UTF-8. All
// of that shipped behind a 200, a green 256/288 suite, and a green cable
// walk, because not one of those gates asks a browser for a file.
//
// A behavioural spec cannot replace this. When Turbo's import 404s, a
// locator waiting for a message row reports "expected 1, got 0" after
// thirty seconds; this reports `404 GET /assets/turbo.js` in one.
//
// It also stays useful after the current gaps close: an import map pin
// is a string in `config/importmap.rb`, and nothing but a browser ever
// checks that a file exists behind it.

test('the sign-in page and the room page load with nothing missing and nothing thrown', async ({ page }) => {
  // BEFORE the first navigation — a listener attached after `goto`
  // misses everything the document itself pulled in.
  const findings = watchForFailures(page)

  await signIn(page)
  await page.goto('/rooms/1')

  // Give the module graph a bounded window to finish resolving. NOT a
  // hard wait on `turbo-cable-stream-source[connected]`: if Turbo failed
  // to boot, that wait times out and the spec dies with a timeout —
  // hiding the 404 that explains it behind the symptom it caused. Wait
  // for the signal if it comes, shrug if it does not, and let the
  // assertions below say what actually went wrong.
  await page.waitForLoadState('load')
  await page
    .locator('turbo-cable-stream-source[connected]')
    .first()
    .waitFor({ state: 'attached', timeout: 10_000 })
    .catch(() => {})

  const all = [
    ...findings.responses,
    ...findings.failed,
    ...findings.console,
    ...findings.errors,
  ]
  const { open, known } = triage(all)

  if (known.length) {
    console.log(`\n  ${known.length} known gap(s) on the way past:\n    ${known.join('\n    ')}`)
  }

  expect(open, `the browser reported ${open.length} unledgered failure(s):\n${report(findings)}`)
    .toEqual([])
})

// Separate test, because it answers a different question and should be
// able to fail on its own: the one above says nothing BROKE, this one
// says the client actually CAME UP. A page can serve every byte and
// still boot no JavaScript at all — which is what "unstyled at 200"
// looked like from the outside for the whole of this app's history.
test('the room page boots its client: Turbo connects and Stimulus is mounted', async ({ page }) => {
  await signIn(page)
  await page.goto('/rooms/1')

  // Campfire's own system-test helper waits for exactly this, and for a
  // count: `assert_selector "turbo-cable-stream-source[connected]",
  // count: 3, visible: false`. Asserting `>= 1` here rather than `== 3`
  // — the count is campfire's fixture-room shape and this account has
  // one room, so pinning 3 would encode a detail this harness does not
  // own. The claim that matters is that the socket connected at all.
  await expect(page.locator('turbo-cable-stream-source[connected]').first())
    .toBeAttached({ timeout: 15_000 })

  // `window.Stimulus` and NOTHING ELSE. A `[data-controller]` element is
  // in the server-rendered HTML whether or not Stimulus ever loaded, so
  // asserting on the attribute passes on a page with no JavaScript at
  // all — a false green in exactly the direction this spec exists to
  // rule out. The app assigns the global itself
  // (`app/javascript/controllers/application.js`: `window.Stimulus =
  // application`), so its presence means the module graph resolved AND
  // ran.
  const stimulusUp = await page.evaluate(() => Boolean(window.Stimulus))
  expect(stimulusUp, 'Stimulus did not mount — the asset spec above names the request that failed').toBe(true)

  // The lazy sidebar frame is the end-to-end proof that Turbo is not
  // merely loaded but DRIVING: `<turbo-frame src="/users/me/sidebar">`
  // is empty in the server's response and only fills when Turbo fetches
  // it. campfire's room list lives in there.
  await expect(page.locator('#user_sidebar a[href^="/rooms/"]').first())
    .toBeVisible({ timeout: 15_000 })
})
