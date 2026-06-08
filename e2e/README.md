# End-to-end (Playwright) smoke tests

Browser-driven smoke tests that exercise *dynamic* behavior the
server-to-server DOM diff (`scripts/compare`) can't reach: Turbo Stream
comment inserts, Action Cable websocket broadcasts, computed Tailwind
styles, and validation re-renders.

Unlike `scripts/compare` (which diffs a target's responses against a live
Rails reference), these tests assert fixed expected values, so there is no
reference server — they run against a single **target** server.

## What's tested

| Spec | Behavior |
|------|----------|
| `index.spec.js`         | the three seeded articles render newest-first with correct comment counts |
| `validation.spec.js`    | a too-short body re-renders `:new` (422) with the error summary |
| `tailwind.spec.js`      | the compiled stylesheet serves and Tailwind utilities apply |
| `turbo_comment.spec.js` | adding a comment inserts the row via Turbo, no full reload |
| `action_cable.spec.js`  | a new comment broadcasts live to another viewer over the websocket |

The expected data (titles, comment counts) matches `db/seed.sql` shipped in
every target archive — article 1 has 2 comments, article 2 has 1, article 3
has 0.

## Running

Driven by `scripts/e2e <target>`, which extracts the published archive,
seeds a fresh DB from `db/seed.sql`, boots the target, and runs these specs
against it. See `scripts/e2e --help`.

`E2E_BASE_URL` overrides the target URL (default `http://localhost:3000`).
`E2E_SKIP` is a space/comma list of spec basenames to skip — the per-target
CI matrix uses it to gate specs a target can't yet satisfy, so the job is
green-able while the gap is still recorded.
