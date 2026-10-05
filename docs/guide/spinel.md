# Compile — Spinel

The compile door produces one native executable from a Rails app: no
interpreter, no gems, no Rails, and by default one SQLite file beside
it — the database, like the target, is a build flag. Roundhouse
emits the Ruby shape of the app — the same shape the `ruby` target
runs on CRuby — as a [`spin`](https://github.com/matz/spinel/blob/master/docs/spin.md)
project, and [Spinel](https://github.com/matz/spinel), Matz's
ahead-of-time Ruby compiler, compiles it to C and then to a binary.

## Why this door, among the compiled targets

Rust, Go, Crystal, Swift, Kotlin and C# are all compiled targets too,
and each passes the same conformance gate on the blog. Pick one of
them when what you want is a codebase in that language for your team
to own from here on. Pick Spinel when what you want is *the Rails
app, compiled*: its behavior is the closest to Rails of any target,
by a distance, and will stay so for the foreseeable future. The
reason is structural. Every other target runs a *translation* of the
framework runtime into that language; the Spinel lane runs the
framework runtime itself — the Ruby that implements Active Record,
Action Controller, Action View and the rest for every target —
compiled as-is. A Rails feature that lands in roundhouse lands there
first and works there fully; the [coverage page](rails-coverage.md)
calls it the Campfire tier for that reason.

## Prerequisites

- **Spinel** — the `spinel` compiler and the `spin` project tool on
  `PATH`. Spinel ships as source snapshots with dated tags; build it
  with `make deps && make && sudo make install` from a checkout or its
  release archive (its README has the details). Each roundhouse
  snapshot names the Spinel release it was tested against in
  [`RELEASES.md`](../../RELEASES.md); Spinel moves quickly, and a
  mismatch in either direction is the first thing to rule out when a
  build fails.
- **A C toolchain** and **SQLite headers** (`libsqlite3-dev`,
  `brew install sqlite`) — the binary links `-lsqlite3`.
- **jemalloc headers** (`libjemalloc-dev`, `brew install jemalloc`).
  The emitted `spin.toml` names jemalloc as the program's allocator,
  and `spin` fails the build rather than quietly producing a slower
  binary: glibc's malloc is a third of a server's CPU under load, and
  a build that silently varies between machines is a benchmark that
  silently compares two binaries. Note the *dev* package — the
  `libjemalloc2` runtime alone links nothing. On Apple Silicon,
  Homebrew installs it under `/opt/homebrew/lib`, which the linker does
  not search by default; `spin` asks `pkg-config` where it is
  (matz/spinel#7219), so with `pkg-config` installed it links as is. A
  Spinel older than that, or a host without `pkg-config`, needs
  `LIBRARY_PATH=$(brew --prefix)/lib` (or `export` it), or the link
  fails with `library 'jemalloc' not found`.
- **Ruby with Bundler** — for the asset step only (`make assets`, below),
  which copies Turbo and Stimulus out of their gems; the binary itself
  needs no Ruby.
- **libvips** (`libvips-dev` to build, `libvips42` to run;
  `brew install vips`) — only when the app declares image variants
  (`has_one_attached` with `variant`), in which case `spin.toml` lists
  `ruby-vips`.
- **Node.js 24+** — when the app builds Tailwind (the asset step runs
  `npx @tailwindcss/cli`), and for the browser end-to-end suite.

## Build and run

```sh
roundhouse --target spinel -o out/spinel /path/to/your/rails/app
cd out/spinel
BUNDLE_ONLY=assets bundle install && BUNDLE_ONLY=assets make assets
spin build
sqlite3 storage/development.sqlite3 < db/seed.sql
./build/bin/<app>
```

`make assets` writes `static/`: the app's stylesheets and JavaScript,
Turbo and Stimulus copied out of their gems, and the Tailwind build
when the app has one. `BUNDLE_ONLY=assets` installs just the Gemfile's
`assets` group for it, so no C-extension gem is compiled for a step
that only copies files. Skip it and the binary still serves every page
— with every stylesheet and script a 404.

`spin build` resolves the manifest's dependencies (bcrypt when the
app uses `has_secure_password`, ruby-vips for variants — native
packages fetched by `spin` from their git sources), compiles the tree
to one C translation unit, and links `build/bin/<app>`, named for the
app. The first build of a Campfire-sized app is a few minutes, most of
it the C compiler on a 12 MB translation unit; clang is about twice as
fast as gcc on it, and `spin` honors `CC`.

The binary serves on `:3000` (`PORT`, or `-p`), with Action Cable at
`/cable`, reading the SQLite file at `storage/development.sqlite3`
and the prebuilt static assets under `static/`. Its server is a green
thread per connection over Spinel's own network layer, scheduled on
one OS worker per core; `SPINEL_WORKERS=N` caps the worker count.
`./build/bin/<app> --help` lists the flags. Leave `--workers` at its
default of 1: the prefork mode is not ready (roundhouse#79), and the
single process already uses every core.

```sh
spin test
```

compiles each `test/*.rb` — the app's own model and controller tests,
translated — to its own binary and diffs its output against a
`.expected` snapshot. The `e2e/` Playwright suite is the same one the
other server targets ship.

## One command: OCRAN

[OCRAN](https://github.com/Largo/ocran) packages Ruby programs for
people who have no Ruby, and its `--roundhouse` mode runs the steps
above as one command:

```sh
ocran --roundhouse path/to/rails/app     # -> ./app-spinel/
```

It transpiles with `roundhouse --target spinel`, runs the asset step,
runs `spin build`, and writes a directory with the binary, the
`static/`, `public/`, `db/` and `config/` files it reads, and a
`storage/` that a rebuild keeps, seeded from `db/seed.sql` on the first
build. When the app is not covered yet it shows the useful part of
`roundhouse check --continue` — the gem census, the first errors, the
unsupported-construct list — instead of a compiler error. It finds
`roundhouse`, `spinel` and `spin` through `ROUNDHOUSE`, `SPINEL` and
`SPIN`, then `PATH`, and prints install steps for whichever is
missing. The prerequisites above still apply.

The mode is experimental and not yet in a released OCRAN (1.4.6 is
older); until the next release, run it from a checkout of its `master`:

```sh
git clone https://github.com/Largo/ocran
ruby -I ocran/lib ocran/exe/ocran --roundhouse path/to/rails/app
```

Its asset step came in with
[largo/ocran#65](https://github.com/Largo/ocran/pull/65); an OCRAN
without it produces a binary whose stylesheets and scripts all 404.
`tests/ocran_contract.rs` pins the command lines and output OCRAN
depends on, so a change on this side that would break it fails here
first.

## Deploying

The binary and what it reads at run time are the whole deployment:
`build/bin/<app>`, `static/`, `public/`, `db/` for the seed,
`config/` for anything the app reads from there, and a writable
`storage/` for the database and any uploaded files. The runtime
libraries it needs are `libsqlite3`, `libjemalloc2`, `libvips42`
if variants are in play, and `ffmpeg` for video previews; and a CA
store (`ca-certificates`) if the app makes https requests of its own
(webhooks, link previews, Web Push).

For a machine without Spinel, `spin pack` writes a directory that
builds from C alone — the generated C, the Spinel runtime as source,
any native package's source, and a Makefile — so the build host needs
a C compiler and `make` and nothing else. That is how the project's
own Campfire Docker archive is built; its two-stage `Dockerfile` is
the template to copy:

```dockerfile
FROM debian:trixie-slim AS build
RUN apt-get update -qq && \
    apt-get install --no-install-recommends -y clang make libsqlite3-dev libjemalloc-dev libvips-dev && \
    rm -rf /var/lib/apt/lists/*
COPY pack /src
RUN make -C /src -j"$(nproc)" CC=clang

FROM debian:trixie-slim
RUN apt-get update -qq && \
    apt-get install --no-install-recommends -y ca-certificates libsqlite3-0 libjemalloc2 libvips42 ffmpeg && \
    rm -rf /var/lib/apt/lists/*
RUN groupadd --system --gid 1000 <app> && \
    useradd <app> --uid 1000 --gid 1000 --no-create-home --shell /usr/sbin/nologin
WORKDIR /app
COPY app/ ./
COPY --from=build /src/<app> ./<app>
RUN mkdir -p storage && chown 1000:1000 storage
VOLUME /app/storage
COPY --chmod=755 boot ./boot
ENV PORT=3000
EXPOSE 3000
CMD ["./boot"]
```

where `pack/` is `spin pack <app> --out pack` and `app/` is the
run-time file set above. The server runs as uid 1000, not root; `boot`
starts as root only long enough to hand a volume an earlier image
created as root to that user:

```sh
#!/bin/sh
set -e
if [ "$(id -u)" = 0 ]; then
  chown -R 1000:1000 /app/storage
  exec setpriv --reuid=1000 --regid=1000 --init-groups ./<app> "$@"
fi
exec ./<app> "$@"
```

A new deployment with no such volume can drop `boot` for `USER
1000:1000` and `CMD ["./<app>"]`. The Campfire image built this way is about
600 MB, nearly all of it libvips and ffmpeg (the binary is 11 MB), and
runs the whole product — sign-in, rooms, uploads, search,
live updates over the socket — from `docker run -p 3000:3000`.
[rubys.github.io/roundhouse/campfire/docker.tgz](https://rubys.github.io/roundhouse/campfire/docker.tgz)
is that archive, refreshed by scheduled full validation or an explicitly
publishing manual run on canonical main. PR archive checks never publish it;
see [CI publication](../ci/README.md#publication-is-separate). It is the fastest way to see the door's
end state before pointing it at your own app.

## What to expect

Spinel is a whole-program compiler with its own type inference, and
the emitted Ruby is written to its subset: no `eval`, no
`method_missing`, no reopening classes at runtime, every method's
types resolvable statically. Roundhouse takes care of that — it is the
whole point of lowering — so an app that transpiles cleanly compiles
cleanly, and when it does not, the failure is in one of two places:

- **`spin build` fails in the compiler.** Spinel could not type
  something roundhouse emitted. That is a bug in one or the other and
  is filed as such; the project's CI tracks Spinel's `master` unpinned
  for exactly this reason, and the error message names the emitted
  file and line.
- **It builds but behaves differently from Rails.** The
  [compare oracle](verifying.md) is the reproduction: `bin/rh compare
  spinel` for the fixture, or `roundhouse-compare` against your own
  app. The Campfire Spinel lane compares every page and every cable frame
  against live Rails in full validation and when its owning inputs select
  it; ordinary native Spinel changes do not automatically run that lane.

The binary's performance is measured continuously on the blog and on
Campfire against Rails as Rails ships it — with its fragment caching
on, jemalloc, and Puma's default configuration — on the same machine:
[rubys.github.io/roundhouse/bench](https://rubys.github.io/roundhouse/bench/).
Read that page rather than a number quoted here; it changes with
every Spinel release and the methodology is written beside the
charts.
