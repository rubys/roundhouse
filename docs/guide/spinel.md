# Compile — Spinel

The compile door produces one native executable from a Rails app: no
interpreter, no gems, no Rails, one SQLite file beside it. Roundhouse
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
  `libjemalloc2` runtime alone links nothing.
- **libvips** (`libvips-dev` to build, `libvips42` to run;
  `brew install vips`) — only when the app declares image variants
  (`has_one_attached` with `variant`), in which case `spin.toml` lists
  `ruby-vips`.
- **Node.js 18+** — for the browser end-to-end suite only.

## Build and run

```sh
roundhouse --target spinel -o out/spinel /path/to/your/rails/app
cd out/spinel
spin build
sqlite3 storage/development.sqlite3 < db/seed.sql
./build/bin/<app>
```

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

## Deploying

The binary and what it reads at run time are the whole deployment:
`build/bin/<app>`, `static/`, `public/`, `db/` for the seed,
`config/` for anything the app reads from there, and a writable
`storage/` for the database and any uploaded files. The runtime
libraries it needs are `libsqlite3`, `libjemalloc2`, and `libvips42`
if variants are in play.

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
    apt-get install --no-install-recommends -y libsqlite3-0 libjemalloc2 libvips42 && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY app/ ./
COPY --from=build /src/<app> ./<app>
RUN mkdir -p storage
VOLUME /app/storage
ENV PORT=3000
EXPOSE 3000
CMD ["./<app>"]
```

where `pack/` is `spin pack <app> --out pack` and `app/` is the
run-time file set above. The Campfire image built this way is about
160 MB and runs the whole product — sign-in, rooms, uploads, search,
live updates over the socket — from `docker run -p 3000:3000`.
[rubys.github.io/roundhouse/campfire/docker.tgz](https://rubys.github.io/roundhouse/campfire/docker.tgz)
is that archive, rebuilt on every push; it is the fastest way to see
the door's end state before pointing it at your own app.

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
  app. The Campfire lane compares every page and every cable frame
  against live Rails on every push.

The binary's performance is measured continuously on the blog and on
Campfire against Rails as Rails ships it — with its fragment caching
on, jemalloc, and Puma's default configuration — on the same machine:
[rubys.github.io/roundhouse/bench](https://rubys.github.io/roundhouse/bench/).
Read that page rather than a number quoted here; it changes with
every Spinel release and the methodology is written beside the
charts.
