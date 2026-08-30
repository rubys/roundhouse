import { expect } from '@playwright/test'

export const EMAIL = process.env.CAMPFIRE_EMAIL || 'e2e@example.com'
export const PASSWORD = process.env.CAMPFIRE_PASSWORD || 'secret123456'

/**
 * Sign in through campfire's own form, the way its own
 * `test/test_helpers/system_test_helper.rb` does: visit root, fill the
 * two fields, submit.
 *
 * BY ID, because campfire's labels carry no text — they wrap the input
 * around an icon (`<label class="flex align-center gap input ...">`),
 * so `getByLabel` matches nothing. Capybara's `fill_in "email_address"`
 * works in campfire's own suite because it falls back to matching the
 * id and the name; Playwright's label locator does not, and reports the
 * miss as a thirty-second timeout with no hint about why.
 *
 * The landing assertion waits for `#user_sidebar`, an element only the
 * SIGNED-IN layout renders, and waits for it to be ATTACHED rather than
 * filled.
 *
 * Both halves of that are load-bearing, and the first cost a debugging
 * session. `#main-content` is on the sign-in page TOO, so asserting on
 * it returned from this helper while the login POST was still in
 * flight; the caller's next `goto` then aborted that POST, and the spec
 * ran against a page that had silently never signed in. A landing
 * assertion has to name something the destination has and the origin
 * does not.
 *
 * Attached, not filled, because `#user_sidebar` is a lazy `<turbo-frame
 * src="/users/me/sidebar">` whose CONTENT arrives only once Turbo
 * boots. Waiting for the content would make signing in depend on the
 * very thing the asset spec exists to check, and every asset failure
 * would surface as a timeout in this helper rather than as the 404 that
 * caused it.
 */
export async function signIn(page) {
  await page.goto('/')
  await page.locator('#email_address').fill(EMAIL)
  await page.locator('#password').fill(PASSWORD)
  await page.locator('button[name="log_in"]').click()
  await expect(page.locator('#user_sidebar')).toBeAttached({ timeout: 15_000 })
}

/**
 * Watch a page for every way the browser can tell us something did not
 * load or did not run, and return the collected findings.
 *
 * Four channels, because they catch different failures and no one of
 * them subsumes the others:
 *
 *   - `response`     — the server answered, badly (a 404 for an import
 *                      map pin, a 500 from a controller).
 *   - `requestfailed` — the request never completed at all (DNS, abort,
 *                      connection reset). A 404 is a *successful*
 *                      request as far as this event is concerned, which
 *                      is why both are needed.
 *   - `console`      — the page's own error output, including the
 *                      module-resolution failures a bare `<script
 *                      type="module">` reports and nothing else does.
 *   - `pageerror`    — an uncaught exception, which is how a Stimulus
 *                      controller that throws on connect surfaces.
 *
 * Attach BEFORE the first navigation; a listener added after `goto`
 * misses everything the document itself pulled in.
 */
export function watchForFailures(page) {
  const findings = { responses: [], failed: [], console: [], errors: [] }

  page.on('response', res => {
    if (res.status() >= 400) {
      findings.responses.push(`${res.status()} ${res.request().method()} ${res.url()}`)
    }
  })
  page.on('requestfailed', req => {
    findings.failed.push(`${req.failure()?.errorText ?? 'failed'} ${req.url()}`)
  })
  page.on('console', msg => {
    if (msg.type() === 'error') findings.console.push(msg.text())
  })
  page.on('pageerror', err => {
    findings.errors.push(String(err))
  })

  return findings
}

/**
 * Known-open gaps, each with the reason it is open.
 *
 * The same split `scripts/campfire-cable-drive.rb` uses: a LEDGER entry
 * is a gap that is written down somewhere, so it reports rather than
 * failing the run — a spec that has been red since the day it was
 * written stops being read. Anything not listed here is a MILESTONE and
 * fails.
 *
 * Every entry must name where the gap is recorded. If you cannot name
 * one, it is not a ledger entry; fix it or write it down first.
 *
 * Deliberately EMPTY on the first run. The room page pins ninety-five
 * modules and links twenty-six stylesheets that no browser has ever
 * requested, so pre-populating this from guesswork would silence
 * findings before anyone had seen them.
 */
export const LEDGER = [
  // { pattern: /example\.js$/, why: 'docs/pipeline/runtime.md § ...' },
]

/** Partition findings into what fails the run and what is ledgered. */
export function triage(lines) {
  const open = []
  const known = []
  for (const line of lines) {
    const entry = LEDGER.find(l => l.pattern.test(line))
    if (entry) known.push(`${line}\n      known open — ${entry.why}`)
    else open.push(line)
  }
  return { open, known }
}

/** A readable dump: every finding, grouped, with its channel named. */
export function report(findings) {
  const sections = [
    ['HTTP >= 400', findings.responses],
    ['request failed', findings.failed],
    ['console error', findings.console],
    ['uncaught exception', findings.errors],
  ]
  return sections
    .filter(([, lines]) => lines.length)
    .map(([name, lines]) => `  ${name} (${lines.length}):\n${lines.map(l => `    ${l}`).join('\n')}`)
    .join('\n')
}
