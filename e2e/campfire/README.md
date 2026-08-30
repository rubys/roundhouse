# Campfire browser specs

Playwright specs that drive the **emitted ONCE Campfire** in a real
browser. Distinct from the suite one directory up: those run against a
published `<target>.tgz` of the blog fixture, these run against a
campfire emit that `scripts/campfire-e2e` builds and boots.

```sh
scripts/campfire-e2e              # emit, make assets, seed, boot, run
scripts/campfire-e2e --keep       # ...and keep the emit
scripts/campfire-e2e --reuse DIR  # re-run against an emit already built
scripts/campfire-e2e --headed     # watch it happen
```

The script owns the whole lifecycle — transpile for `ruby`, `make
assets`, seed the schema, boot Puma on a free port, complete the app's
own `/first_run` — and hands the specs a `CAMPFIRE_BASE_URL`. There is
no fixture data: `/first_run` is campfire's own onboarding and creates
the account, the first user and the first room.

## What's tested

| Spec | Behavior |
|------|----------|
| `assets.spec.js` | signing in and opening a room requests nothing that 404s and throws nothing; the room page then boots Turbo, mounts Stimulus, and fills its lazy sidebar frame |

## Why the first spec is a network spec

The room page pins 95 ES modules and links 26 stylesheets. Until
`18e1601f` the build produced **none** of them, and that shipped behind
a 200, a green 256/288 conformance run, and a green
`scripts/campfire-cable-walk` — because not one of those gates asks a
browser for a file.

A behavioural spec cannot replace this one. When Turbo's import 404s, a
locator waiting for a message row reports `expected 1, got 0` after
thirty seconds; `assets.spec.js` reports `404 GET /assets/turbo.js`
immediately. Ablate it to see the difference — `mv static static.off`
in a kept emit and re-run with `--reuse`: 405 named 404s.

It also keeps earning its place after those gaps close. An import map
pin is a string in `config/importmap.rb`, and nothing but a browser ever
checks that a file exists behind it.

## Relationship to `campfire-cable-walk`

They assert different halves and neither is a superset.

| | server | client |
|---|---|---|
| `tests/overlay_cable_*` | objects | objects |
| `scripts/campfire-cable-walk` | real | **synthetic** |
| these specs | real | real |

The walk's synthetic client is a feature, not a limitation: it subscribes
to the stock `Turbo::StreamsChannel` with a stolen-but-valid signed
stream name and requires a rejection — campfire's
`RoomStreamsAreAuthorized` guard, end to end. Campfire's own JavaScript
never sends that frame, so no browser test can make that assertion, in
the same way a real browser cannot test what your API does with a
malformed request.

## Ledger entries

`helpers.js` exports a `LEDGER` array. An entry downgrades a finding
from a failure to a reported note, and **must name where the gap is
written down** (normally `docs/pipeline/runtime.md`) — the same rule
`scripts/campfire-cable-drive.rb` uses for its ledger probes, and for
the same reason: a spec that has been red since the day it was written
stops being read. If you cannot name the entry, it is not a ledger item.

It is empty today. Keep it that way where you can.
