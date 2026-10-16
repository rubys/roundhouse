# scripts/lib/trim-logs.sh — cut every oversized file in a run's log
# directory down to its tail, for the bench harnesses' exit traps.
#
# WHY THIS EXISTS. A lane log under load is a request log: Rails writes a
# line per request and Thruster writes another, so one campfire rails lane
# left a 3.1 GB `rails.log` in every results directory, and the blog
# bench's `rails-truffleruby-1w.log` reached 5.4 GB in a `mktemp -d` that
# nothing ever removed. On 2026-10-15 the box's disk filled (32 blog-bench
# log dirs = 185 GB in /tmp, 41 GB of campfire results), a campfire POST
# cell died on `SQLite3::FullException`, and the lane it was measuring
# outlived its supervisor holding the port.
#
# TRIMMED, NOT DELETED. What anyone reads from these logs after a run is
# the end: why a lane failed to boot, the error behind a non-2xx count
# ("see the lane log"), the last request before a crash. The tail keeps
# that; the gigabytes of 200s ahead of it are not evidence of anything.
# Small files (wrk output, rss samples, captured bodies) are left alone.
#
# Usage: trim_logs DIR [MAX_BYTES]   (default 1 MiB per file)
trim_logs() {
    local dir="$1" max="${2:-1048576}" f size
    [[ -n "$dir" && -d "$dir" ]] || return 0
    for f in "$dir"/*; do
        [[ -f "$f" ]] || continue
        size="$(wc -c < "$f" 2>/dev/null | tr -d ' ')" || continue
        (( size > max )) || continue
        { printf '[trimmed by trim_logs: %s bytes, last %s kept]\n' "$size" "$max"
          tail -c "$max" "$f"; } > "$f.trim" 2>/dev/null \
            && mv -f "$f.trim" "$f" \
            || rm -f "$f.trim"
    done
    return 0
}
