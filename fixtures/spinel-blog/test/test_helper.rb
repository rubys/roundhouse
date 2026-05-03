# Bridge: canonical test_helper lives at runtime/spinel/test/ in the
# workspace (one source of truth, overlaid into the emitted demo by
# `make spinel-transpile` and into the toolchain test's scratch dir
# by `tests/spinel_toolchain.rs`). For the standalone spinel-blog
# fixture (run from the workspace), this bridge routes
# `require_relative "../test_helper"` to the canonical path so the
# hand-written tests under fixtures/spinel-blog/test/ keep working
# without divergence.
#
# Sets ROOT here so the canonical computes LOAD_PATH against this
# fixture's tree (not the canonical's own location under runtime/).
# Same bridging pattern as fixtures/spinel-blog/runtime/active_record.rb.
ROOT = File.expand_path("..", __dir__)
require_relative "../../../runtime/spinel/test/test_helper"
