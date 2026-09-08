# scripts/lib/spinel-link-path.sh — put the allocator's library on the
# compiler's search path, for scripts that build an emitted spinel tree.
#
# WHY THIS EXISTS. Every spinel emit's spin.toml names an allocator
# (`allocator = "jemalloc"`, written unconditionally by spin_shape in
# src/project.rs), and `spin` turns that into a bare `--link -ljemalloc`
# with no `-L`: the manifest states what the program needs and leaves
# finding it to the environment (spinel docs/spin.md, "Building outside
# spin"). On linux the distro package lands in /usr/lib, which the linker
# already searches, so the lanes there were always fine. On a mac it lands
# in homebrew's prefix, which clang does NOT search -- the default library
# search path on a current Xcode is the SDK's usr/lib and nothing else, not
# /opt/homebrew/lib and not even /usr/local/lib -- so every script that
# builds an emit died at
#
#     ld: library 'jemalloc' not found
#     spinel: C compilation failed
#
# and had done since 60d4e2e3 named the allocator. There is no way to
# install jemalloc that fixes this: homebrew installs into its own prefix
# by design and the compiler does not look there.
#
# LIBRARY_PATH IS THE VARIABLE, NOT LDFLAGS. LIBRARY_PATH is read by the
# compiler driver itself, so it reaches spinel, `spin`, a hand-run `cc`,
# and anything else that links through one -- including rustc, which shells
# out to cc and inherits it without knowing the variable exists. LDFLAGS is
# a build-system convention that make and autoconf interpolate into their
# own link commands; nothing propagates it downward, and neither spinel nor
# `spin` reads it.
#
# Usage: source it once, call it before the build.
#
#     . "$REPO_ROOT/scripts/lib/spinel-link-path.sh"
#     spinel_link_path
#
# A no-op on linux, where neither directory exists. Composed here rather
# than left to the caller's profile because a lane that only builds for
# whoever knows the trick is not a lane -- and because CI and a fresh clone
# have no profile to inherit.

spinel_link_path() {
    local d
    for d in /opt/homebrew/lib /usr/local/lib; do
        [[ -d "$d" ]] || continue
        # Idempotent: a second call, or a caller whose profile already set
        # it, must not grow the variable.
        case ":${LIBRARY_PATH:-}:" in
            *":$d:"*) continue ;;
        esac
        LIBRARY_PATH="${LIBRARY_PATH:+$LIBRARY_PATH:}$d"
    done
    # Export only with a value. gcc reads an EMPTY element of LIBRARY_PATH
    # as `.`, so exporting an empty one on linux -- where neither directory
    # exists and the system jemalloc is already on the default path -- would
    # quietly put the working directory on the link line.
    if [[ -n "${LIBRARY_PATH:-}" ]]; then
        export LIBRARY_PATH
    fi
    # Never the caller's exit status: these scripts run under `set -e`, and
    # a linux miss is a no-op, not a failure.
    return 0
}
