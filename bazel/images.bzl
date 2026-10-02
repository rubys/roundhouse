"""The remote executors' CI image; bump the tag whenever bazel/images/all/Dockerfile or the scaffold Gemfile it bundles changes."""

# Not a moving tag: an executor caches an image by name and keeps running the stale one.
CI_IMAGE = "docker://ghcr.io/rubys/roundhouse-ci-all:v1"

# The docker lane's Firecracker VM image (bazel/images/dind/Dockerfile).
DIND_IMAGE = "docker://ghcr.io/rubys/roundhouse-ci-dind:v1"

# once-campfire at the commit ci.yml pins.
CAMPFIRE_SHA = "90b330024dec3e757c79b6a7e6568f93da8e3148"

# The other pinned sources ci.yml fetches.
WRITEBOOK_SHA = "f3fadd21907ad9b18cb23800d971c2cc25045e2a"
MASTODON_SHA = "163f96cee4dea23365bff9b433871e68d20d9ee7"
RUBY_BENCH_SHA = "d771f81f1ce9db51376e03ca7b8e6a83160556d4"
