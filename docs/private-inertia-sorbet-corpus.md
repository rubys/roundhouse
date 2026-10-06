# Private Inertia + Sorbet corpus hillclimb

A large private Rails 8.1 + Inertia + Sorbet app is used as a
**forcing corpus** for Roundhouse coverage — the same role Writebook
plays for class-body macros ([`writebook.md`](writebook.md)), not a
support claim that the private app itself emits or runs.

Do not name private products or clients in this public tree. Measure
with `roundhouse check --continue` against a local checkout of that
corpus. Numbers below are a baseline on `rubys/roundhouse` `main` @
`8d84f074` (2026-10-06):

| Metric | Count |
|---|---:|
| Parse errors | 0 |
| Errors | 276 |
| Warnings | 2160 |
| Gap-attributed notes | 153 |
| Survey gaps | 47 (15 kinds) |
| Unknown gems | 27 |

Re-measure after climbs; ratchet the honest ledger, do not suppress
diagnostics to green a check.

## Already tracked elsewhere — do not duplicate

| Theme | Existing |
|---|---|
| General Sorbet / RBS / AR / stdlib batch from another internal app | [#503](https://github.com/rubys/roundhouse/pull/503) (open Meta PR 2), [#286](https://github.com/rubys/roundhouse/pull/286) (merged) |
| Writebook class-body / delegated_type | [#30](https://github.com/rubys/roundhouse/issues/30), [#500](https://github.com/rubys/roundhouse/pull/500) |
| Dry::Struct lowering (parallel to `T::Struct`) | [#486](https://github.com/rubys/roundhouse/pull/486) |
| Alba attribution | [#207](https://github.com/rubys/roundhouse/issues/207), [#142](https://github.com/rubys/roundhouse/issues/142) |
| `**rest` beside keywords | [#388](https://github.com/rubys/roundhouse/issues/388), [#448](https://github.com/rubys/roundhouse/pull/448) |
| Engine mounts fail-loud | [#396](https://github.com/rubys/roundhouse/issues/396), [#397](https://github.com/rubys/roundhouse/pull/397) |
| Nilable binops | [#394](https://github.com/rubys/roundhouse/issues/394) |
| Kaminari pagination (corpus uses Pagy — related pattern) | [#311](https://github.com/rubys/roundhouse/issues/311) |
| Gap-cascade attribution | [#481](https://github.com/rubys/roundhouse/pull/481) |
| Sorbet replay / `type_member` | [#125](https://github.com/rubys/roundhouse/issues/125), [#113](https://github.com/rubys/roundhouse/issues/113) |

## Climb themes (priority)

### P0.1 — `Date.current` and ActiveSupport calendar on `Date`

Baseline: **47** `no known method current on Date`, plus `yesterday` /
`beginning_of_month` / `in_time_zone` on Date receivers.

`Date.today` is already accepted by `date_constructor`; Rails'
`Date.current` (zone-aware) was missing. Instance calendar helpers live
on the Time table but not on `date_method`.

First commit on this branch adds `Date.current` → `Ty::Date`. Follow-ups
extend AS calendar methods onto Date with runtime + `emit_and_run`.

### P0.2 — `T::Struct` / `const` (Period VO and ~261 structs)

Survey noise: `const` as unrecognized controller class-body macro on
`HasPeriodParams::Period`. Align with Dry::Struct work (#486); do not
fork a second lowering path. Pin with `emit_and_run` before clearing
errors (invariant 6).

### P0.3 — Inertia Rails

Survey: `inertia_config`, `inertia_share`; unknown gem `inertia_rails`;
~100 `render inertia:` sites. Start with honest attribution (Alba
pattern in #207), then bounded recognition of `render inertia:` /
share props — no React emit claim.

### P0.4 — Concern class-method macros (`period_config` ×24)

Generalize concern macro expansion (#30 / Writebook `positioned_within`
lane), not a corpus-only special case.

### P1 — Forwarding, routes, jobs, AR constants, binops, Pagy

See links in the table above. Prefer landing or extending those PRs
over new issues when the symptom matches.

## Child PR rules

- Open every PR against **`rubys/roundhouse` `main`** (never against a
  fork as the base repository).
- Features land once in shared ingest/lower/runtime — never per-target
  copies (`AGENTS.md` invariants).
- Clearing an error is a support claim: pair with `emit_and_run` when
  the construct becomes executable.
- Keep private product and client names out of public Roundhouse
  titles, bodies, docs, and commit messages.

## Out of scope for this climb

- Emitting or shipping the private app itself.
- Modeling private domain gems or hardcoding app timezone constants
  (app initializers must resolve normally; see survey on dropped
  constants).
- Duplicating #503's already-open Sorbet/RBS/AR stack.
