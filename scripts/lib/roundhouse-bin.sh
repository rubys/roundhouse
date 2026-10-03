# scripts/lib/roundhouse-bin.sh — resolve the roundhouse compiler executable.
#
# Prefer ROUNDHOUSE_BIN when set (CI stages the unit job's current-run debug
# binary). Otherwise fall back to `cargo run --quiet --bin roundhouse` so a
# local checkout keeps working with no prebuilt requirement.
#
# Downloading a binary alone does not make `cargo run` consume it: every
# harness that should share the producer must call roundhouse_run rather than
# hard-coding cargo.
#
# Both branches run with cwd = REPO_ROOT when REPO_ROOT is set, so relative
# input/output paths mean the same thing whether CI supplied a binary or the
# local path rebuilds through cargo. A relative ROUNDHOUSE_BIN is resolved
# against the caller's cwd *before* that chdir, so the checked path is the
# one that runs.
#
# Usage (after REPO_ROOT is set):
#
#     . "$REPO_ROOT/scripts/lib/roundhouse-bin.sh"
#     roundhouse_run --target ruby "$APP" -o "$OUT" --allow-unsupported
#
# When ROUNDHOUSE_BIN is set, a missing or non-executable path fails hard —
# never silently rebuild through cargo. ROUNDHOUSE_BIN_TRACE=1 prints the
# resolved argv to stderr before exec (CI uses this to prove consumption).

roundhouse_run() {
    local -a cmd
    local bin
    if [[ -n "${ROUNDHOUSE_BIN:-}" ]]; then
        bin="$ROUNDHOUSE_BIN"
        # Resolve relative paths against the caller's cwd before chdir to
        # REPO_ROOT; otherwise ./fake can pass the check and miss at exec.
        if [[ "$bin" != /* ]]; then
            bin="$(pwd)/$bin"
        fi
        if [[ ! -f "$bin" || ! -x "$bin" ]]; then
            printf 'roundhouse-bin: ROUNDHOUSE_BIN is not an executable file: %s\n' \
                "$bin" >&2
            return 127
        fi
        cmd=("$bin")
    else
        if [[ -z "${REPO_ROOT:-}" ]]; then
            printf 'roundhouse-bin: REPO_ROOT is unset and ROUNDHOUSE_BIN is empty\n' >&2
            return 127
        fi
        cmd=(cargo run --quiet --bin roundhouse --)
    fi
    if [[ -n "${ROUNDHOUSE_BIN_TRACE:-}" ]]; then
        printf 'roundhouse-bin: exec' >&2
        printf ' %q' "${cmd[@]}" "$@" >&2
        printf '\n' >&2
    fi
    # Same cwd for both branches: cargo historically ran under REPO_ROOT, and
    # a prebuilt binary must resolve relative paths the same way.
    if [[ -n "${REPO_ROOT:-}" ]]; then
        (cd "$REPO_ROOT" && "${cmd[@]}" "$@")
    else
        "${cmd[@]}" "$@"
    fi
}
