#!/usr/bin/env python3
"""Select coverage, not test results.

Two gates: merge-gate is the Ruby floor (BASE) plus any focus lanes the
author made required; the Full/extra ledger can stay red without holding
merge. Extra-language SDKs need a path owner, a focus label, `ci:full`,
or scheduled full validation. Spinel follows the same path/focus/Full
ladder on PRs and on pushes to main — there is no always-on full Spinel
suite for unrelated main merges. Unknown inputs keep the Ruby floor plus
advisory Spinel (unless a focus label is already set).
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time
from collections import namedtuple
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

TARGETS = [
    "rust",
    "crystal",
    "kotlin",
    "swift",
    "csharp",
    "typescript",
    "go",
    "elixir",
    "python",
    "ruby",
    "jruby",
]
# compare-extra / smoke-extra matrix cells for the seven emitted SDK langs.
# Order matches TARGETS. Rust/TS ride the separate `compare` job; ruby /
# jruby are the interpreted floor / compare-jruby lane.
EXTRA_COMPARE_TARGETS = [
    t for t in TARGETS if t not in {"rust", "typescript", "ruby", "jruby"}
]
COMPARE_TARGETS = ["rust", "typescript"]
FLOOR_SMOKE_TARGETS = ["rust", "typescript", "ruby", "jruby"]
# PR floor: the Ruby shape plus Campfire. Extra languages, Rust/TS
# compare, WASM, and Spinel are not in this list.
BASE = [
    "generate-fixture",
    "unit-build",
    "unit",
    "build-roundhouse",
    "store-check",
    "compare-ruby",
    "compare-ruby-next",
    "campfire-conformance",
    "campfire-compare",
]
# Compact publication additionally waits on Rust/TS when those jobs were
# selected (scheduled full, or a change that owns them). Unselected skips
# must not fail the Ruby PR floor.
PUBLICATION = [*BASE, "compare", "browser-smoke-typescript"]
CORE = ["spinel-build", "spinel-toolchain", "spinel-compare"]
JRUBY_BIND_INPUTS = {
    "tests/param_binds_jruby.rb",
    "tests/support/jdbc_value_semantics.rb",
    "tests/support/jdbc_cleanup_failures.rb",
    "tests/param_binds_runtime.rb",
    "tests/param_binds_nil.rb",
    "tests/param_binds_values.rs",
    "tests/param_binds_values.rb",
    "tests/param_binds_cleanup.rs",
    "tests/param_binds_cleanup.rb",
    "runtime/spinel/test/statement_cache_cases.rb",
}
PARAM_BIND_TESTS = ["param_binds", "param_binds_values", "param_binds_planner", "param_binds_cleanup"]
SPINEL_TESTS = [
    "date_columns_spinel",
    "temporal_initialize_spinel",
    "framework_tests_spinel",
    "spinel_web_push_crypto",
    "openssl_random",
    "spinel_db_lease",
    *PARAM_BIND_TESTS,
    "spinel_stmt_cache_lru",
    "stmt_stats",
    "db_sqlite_concurrency",
    "spinel_param_builder",
    "spinel_net_http_start",
    "rails_compat_vectors_spinel",
    "spinel_pg_db",
    "spinel_db_contract",
    "generated_columns_spinel",
    "postgres_json_types_spinel",
    "array_column_storage_spinel",
    "pessimistic_locking",
    "not_found_parity_spinel",
    "spinel_transaction_connection",
    "routes_segment_pattern_spinel",
    "open_telemetry_stub_spinel",
    "spinel_mini_profiler",
    "spinel_active_support_instrumentation",
    "around_block_filter_spinel",
    "after_initialize_bare_new_spinel",
    "find_or_create_bang_records",
    "before_action_return_spinel",
    "partial_flash_from_caller_spinel",
    "plain_class_optional_keywords_spinel",
    "jbuilder_array_value_spinel",
    "active_job_payload",
    "render_json_encoder_spinel",
    "url_helpers_absolute_spinel",
    "jbuilder_extract_attributes_spinel",
    "hash_to_query_symbol_keys_spinel",
    "storage_proxy_path_spinel",
    "app_reopens_runtime_class",
    "filter_block_params_spinel",
    "sidekiq_push",
    "sidekiq_worker",
    "model_queries_spinel",
    "default_scope_spinel",
    "default_scope_dynamic_spinel",
    "acts_as_tenant_spinel",
    "typed_writer_cast_spinel",
    "pundit_source_spinel",
    "row_from_raw_hash_spinel",
    "helper_method_instance_state_spinel",
    "render_relation_chain",
    "render_object_locals",
    "rails_cache_entry_options_spinel",
]
# Inputs of the Sidekiq push gate (tests/sidekiq_push.rs): the files its
# driver loads beside it.
SIDEKIQ_PUSH_INPUTS = {
    "runtime/ruby/active_job.rb",
    "runtime/spinel/active_job_serialization.rb",
    "runtime/spinel/global_id_locator.rb",
    "runtime/spinel/base64.rb",
    "runtime/spinel/sidekiq_push.rb",
    "runtime/spinel/sidekiq_redis.rb",
    "runtime/spinel/job_registry.rb",
}
# Inputs of the Sidekiq worker gate (tests/sidekiq_worker.rs): the push
# runtime its drivers load, plus the envelope and the worker role.
SIDEKIQ_WORKER_INPUTS = SIDEKIQ_PUSH_INPUTS | {
    "runtime/spinel/sidekiq_envelope.rb",
    "runtime/spinel/sidekiq/sidekiq_worker.rb",
}
# Inputs of the live Sidekiq gates (tests/sidekiq_push_live.rs and
# tests/sidekiq_worker_live.rs, the sidekiq-live job): the push runtime,
# the worker, their real Redis client, and the gates.
SIDEKIQ_LIVE_INPUTS = (SIDEKIQ_WORKER_INPUTS - {"runtime/spinel/sidekiq_redis.rb"}) | {
    "runtime/spinel/sidekiq/sidekiq_redis.rb",
    "tests/sidekiq_push_live.rs",
    "tests/sidekiq_worker_live.rs",
    "runtime/spinel/sidekiq/redis_cable.rb",
    "tests/redis_cable_live.rs",
    "tests/redis_cable/rails.json",
}
# Seconds each suite took in spinel-framework (main run 38936441376, its
# test binary's compile included), for balancing its shards; a suite not
# listed counts as one Spinel build.
SPINEL_TEST_SECONDS = {
    "spinel_transaction_connection": 501,
    "spinel_stmt_cache_lru": 203,
    # Two Spinel compiles and a run; measured in #1455's CI run 40602974151.
    "row_from_raw_hash_spinel": 183,
    "param_binds": 181,
    "not_found_parity_spinel": 169,
    "param_binds_values": 126,
    "param_binds_cleanup": 86,
    "framework_tests_spinel": 78,
    "date_columns_spinel": 62,
    "postgres_json_types_spinel": 61,
    "around_block_filter_spinel": 57,
    "pessimistic_locking": 54,
    "render_json_encoder_spinel": 54,
    "jbuilder_array_value_spinel": 52,
    "jbuilder_extract_attributes_spinel": 52,
    "hash_to_query_symbol_keys_spinel": 51,
    "open_telemetry_stub_spinel": 50,
    "storage_proxy_path_spinel": 46,
    "plain_class_optional_keywords_spinel": 41,
    "url_helpers_absolute_spinel": 38,
    "routes_segment_pattern_spinel": 37,
    "generated_columns_spinel": 36,
    "db_sqlite_concurrency": 23,
    "spinel_pg_db": 19,
    "spinel_net_http_start": 10,
    **dict.fromkeys(
        [
            "spinel_web_push_crypto",
            "spinel_db_lease",
            "param_binds_planner",
            "spinel_param_builder",
            "rails_compat_vectors_spinel",
            "active_job_payload",
            "sidekiq_push",
            "sidekiq_worker",
        ],
        5,
    ),
}
SPINEL_TEST_DEFAULT_SECONDS = 40
SPINEL_FRAMEWORK_SHARDS = 4
# Not balanced by count: these three take two to three times a typical test (local run, 2026-10-17).
SPINEL_TOOLCHAIN_SECONDS = {
    "real_blog_spinel_tests_pass": 265,
    "identical_cli_emission_reuses_native_build_but_changed_ruby_rebuilds": 187,
    "finite_concern_class_configuration_runs_natively": 133,
}
SPINEL_TOOLCHAIN_DEFAULT_SECONDS = 80
SPINEL_TOOLCHAIN_SHARDS = 4
SPINEL_TOOLCHAIN_THREADS = 4
# tiny_api's gate runs alone after the suite on the last shard, so its seconds count once per test thread.
SPINEL_TOOLCHAIN_TAIL = "tiny_api"
SPINEL_TOOLCHAIN_TAIL_SECONDS = 111
# The tables above are seeds and the fallback only: main's runs keep measured seconds in the Actions cache (ci.yml spinel-timings).
SPINEL_TIMING_KINDS = ("framework", "toolchain")
# Inputs of the ActiveJob payload gate (tests/active_job_payload.rs):
# the files its driver loads beside it.
ACTIVE_JOB_PAYLOAD_INPUTS = {
    "runtime/ruby/active_job.rb",
    "runtime/spinel/active_job_serialization.rb",
    "runtime/spinel/global_id_locator.rb",
    "runtime/spinel/base64.rb",
}
# Inputs of the PostgreSQL Db gate (tests/spinel_pg_db.rs): the shim, its
# RBS, the contract and time parsing it compiles with, and the cases.
PG_DB_INPUTS = {
    "runtime/spinel/db_pg.rb",
    "runtime/spinel/db_pg.rbs",
    "runtime/spinel/pg_errors.rb",
    "runtime/spinel/pg_errors.rbs",
    "runtime/spinel/pg_driver.rb",
    "runtime/spinel/pg_driver.rbs",
    "runtime/ruby/db.rbs",
    "runtime/spinel/active_support_time_parsing.rb",
    "runtime/spinel/active_support_time_parsing.rbs",
}
# The Db contract suite (tests/support/db_contract.rs): the Spinel lanes
# each of its files feeds, and the runtime files its LANES copy beside a
# shim that nothing above routes to them. cruby_prelude.rb feeds only the
# CRuby lane, which the unit job runs.
DB_CONTRACT_INPUTS = {
    "tests/db_contract/support.rb": ("spinel_db_contract", "spinel_pg_db"),
    "tests/db_contract/contract_cases.rb": ("spinel_db_contract", "spinel_pg_db"),
    "tests/db_contract/spinel_prelude.rb": ("spinel_db_contract", "spinel_pg_db"),
    "tests/db_contract/sqlite_main.rb": ("spinel_db_contract",),
    "tests/db_contract/postgres_cases.rb": ("spinel_pg_db",),
    "tests/db_contract/postgres_main.rb": ("spinel_pg_db",),
    "tests/db_contract/db_pg_lane.rb": ("spinel_pg_db",),
    "tests/support/db_contract.rs": ("spinel_db_contract", "spinel_pg_db"),
    "tests/support/pg_test_url.rs": ("spinel_pg_db",),
    "runtime/spinel/active_support_interval.rb": ("spinel_db_contract", "spinel_pg_db"),
    "runtime/spinel/active_support_duration.rb": ("spinel_db_contract", "spinel_pg_db"),
    "runtime/spinel/active_support_interval.rbs": ("spinel_pg_db",),
    "runtime/spinel/active_support_duration.rbs": ("spinel_pg_db",),
}
GENERATED_COLUMNS_SPINEL_INPUTS = {
    "src/emit/ruby/library.rs",
    "src/emit/shared/schema_sql.rs",
    "src/schema.rs",
    "src/schema/generated.rs",
    "src/ingest/schema.rs",
    "src/ingest/structure_sql.rs",
    "src/lower/persistence.rs",
    "src/lower/generated_write_guard.rs",
    "src/lower/model_to_library/mod.rs",
    "src/lower/model_to_library/row.rs",
    "src/lower/model_to_library/schema.rs",
    "tests/support/emit_and_run.rs",
}
# Inputs of the reopened Net::HTTP gate (tests/spinel_net_http_start.rs):
# the reopen and the two stub tables it compiles with.
NET_HTTP_INPUTS = {
    "runtime/spinel/net_http.rb",
    "runtime/spinel/http_stub.rb",
    "runtime/spinel/http_stub.rbs",
    "runtime/spinel/tcp_socket_stub.rb",
    "runtime/spinel/tcp_socket_stub.rbs",
}
# Inputs of the reopen gate (tests/app_reopens_runtime_class.rs): the
# pass, and the runtime job whose override it builds natively.
APP_REOPENS_INPUTS = {
    "src/emit/ruby/reopen.rs",
    "runtime/ruby/active_storage.rb",
    "runtime/ruby/active_storage.rbs",
}
JSON_TYPES_SPINEL_INPUTS = {
    "src/schema.rs",
    "src/ingest/schema.rs",
    "src/ingest/structure_sql.rs",
    "src/ingest/model.rs",
    "src/emit/shared/schema_sql.rs",
    "src/lower/arel/ruby_values.rs",
    "src/lower/model_to_library/mod.rs",
    "src/lower/model_to_library/schema.rs",
}

# The stored PostgreSQL array column: its pass, storage and runtime seam.
ARRAY_COLUMN_SPINEL_INPUTS = {
    "src/schema.rs",
    "src/ingest/schema.rs",
    "src/ingest/model.rs",
    "src/lower/array_columns.rs",
    "src/emit/shared/schema_sql.rs",
    "src/lower/model_to_library/mod.rs",
    "src/lower/model_to_library/schema.rs",
    "runtime/spinel/array_column.rb",
    "runtime/spinel/array_column.rbs",
}

# Bulk advisory Spinel suite (unknown-input spinel_lane, ruby-family
# project.rs, and Full). Membership drifts — the name is not a job count.
SPINEL = [
    "spinel-build",
    "spinel-framework",
    "campfire-spinel-build",
    "campfire-spinel-compare",
    "campfire-spinel-db",
    "spinel-toolchain",
    "spinel-compare",
    "spinel-smoke",
    "campfire-archive-build",
    "campfire-smoke-docker",
]
# Path-owned or Full-only; not in the bulk SPINEL / spinel_lane suite.
SPINEL_FULL_OR_PATH = ("campfire-smoke", "sidekiq-live")
# Unknown-input / explicit spinel_lane advisory suite. PR `ci:spinel` uses the
# narrower CORE focus lane (built in focus_plan) and makes those jobs required.
# Main pushes no longer force this lane: path ownership, `ci:spinel`, or Full
# (`full-ci` / `ci:full` / `[ci full]`) select Spinel instead.
SPINEL_LANE = [*BASE, *SPINEL, "build-site", "archive-results"]
ADVISORY = (set(SPINEL) | set(SPINEL_FULL_OR_PATH)) - {"campfire-archive-build"}
# Extra-language ledger jobs: advisory on Full/path unless a focus label
# makes them required for a fix round.
LEDGER_EXTRAS = {"compare-extra", "smoke-extra"}
# CORE (+ framework suite) can be hard under ci:spinel focus; workflow CoE
# follows plan spinel-advisory the same way extras-advisory flips extras.
SPINEL_FOCUS_HARD = set(CORE) | {"spinel-framework"}
# Packaging evidence report only. Selected for completeness and required by
# assemble-site when publishing; never a hard CI-summary / compact failure.
# GitHub can mark the job `abandoned` (queued, never assigned) on large
# ci:full PR graphs even when every producer succeeded — that must not red
# an otherwise green PR summary.
REPORTING = {"archive-results"}
SHA = re.compile(r"[0-9a-f]{40}\Z")
PROJECT_BUILDERS = {
    "ruby_runtime_files": "interpreted",
    "jruby_runtime_files": "interpreted",
    "ruby_family_runtime_files": "interpreted",
    "spinel_files": "ruby-family",
    "spin_shape": "ruby-family",
}

# Coverage labels. Precedence: ci:full (or CI_FULL) > any focus labels
# (ci:<lang> / ci:extras / ci:jruby / ci:spinel / ci:rust /
# ci:typescript) > path ownership.
# Focus mode is NARROW: BASE + selected focus lanes only.
CI_FULL = "ci:full"
CI_SPINEL = "ci:spinel"
CI_JRUBY = "ci:jruby"
CI_EXTRAS = "ci:extras"
CI_FOCUS_BY_LABEL = {f"ci:{t}": t for t in EXTRA_COMPARE_TARGETS}
CI_COMPARE_BY_LABEL = {f"ci:{t}": t for t in COMPARE_TARGETS}

CoverageLabels = namedtuple(
    "CoverageLabels", "full focus_spinel focus_jruby focus_extras focus_compare"
)


# A committer pushing to main has no PR to label. A line reading exactly
# `[ci full]` in a pushed commit's message plans like `ci:full`. Only a
# whole line counts, so prose that names the marker does not trigger it,
# and only a push to main honors it: a PR author controls their messages,
# and `ci:full` there stays a maintainer's label.
CI_FULL_MARKER = "[ci full]"


def full_marker_commit(event, event_name, ref):
    """The first pushed commit whose message requests full validation."""
    if event_name != "push" or ref != "refs/heads/main":
        return None
    commits = event.get("commits") or [event.get("head_commit") or {}]
    for commit in commits:
        lines = (commit.get("message") or "").splitlines()
        if any(line.strip() == CI_FULL_MARKER for line in lines):
            return commit.get("id") or ""
    return None


def parse_coverage_labels(names, *, env_full=False):
    """Interpret PR/env coverage labels into a structured request."""
    labels = set(names)
    full = bool(env_full) or CI_FULL in labels
    focus = set()
    if CI_EXTRAS in labels:
        focus.update(EXTRA_COMPARE_TARGETS)
    for label, target in CI_FOCUS_BY_LABEL.items():
        if label in labels:
            focus.add(target)
    focus_extras = tuple(t for t in EXTRA_COMPARE_TARGETS if t in focus)
    focus_compare = tuple(
        target for label, target in CI_COMPARE_BY_LABEL.items() if label in labels
    )
    return CoverageLabels(
        full=full,
        focus_spinel=CI_SPINEL in labels,
        focus_jruby=CI_JRUBY in labels,
        focus_extras=focus_extras,
        focus_compare=focus_compare,
    )


def focus_plan(extras=(), jruby=False, spinel=False, compare_targets=()):
    """BASE plus selected focus lanes; path ownership suppressed.

    Focused extras / jruby / rust / typescript / CORE Spinel are
    merge-gate required for the fix round. Unrelated extras, WASM, and
    the heavy Spinel Campfire suite stay off. `ci:rust` and
    `ci:typescript` add only that language's `compare` and floor `smoke`.
    """
    extra = [t for t in EXTRA_COMPARE_TARGETS if t in extras]
    compare = [t for t in COMPARE_TARGETS if t in compare_targets]
    jobs = list(BASE)
    smoke_floor = []
    reasons = []
    if extra:
        jobs.extend(["compare-extra", "build-site", "smoke-extra", "archive-results"])
        reasons.append(
            "ci focus: BASE + "
            + ", ".join(extra)
            + " (compare-extra and smoke-extra; required)"
        )
    if compare:
        jobs.append("compare")
        if "build-site" not in jobs:
            jobs.extend(["build-site", "archive-results"])
        if "smoke" not in jobs:
            jobs.append("smoke")
        smoke_floor.extend(compare)
        reasons.append(
            "ci focus: "
            + ", ".join(compare)
            + " (compare and floor smoke; required)"
        )
    if jruby:
        if "build-site" not in jobs:
            jobs.extend(["build-site", "archive-results"])
        jobs.append("compare-jruby")
        if "smoke" not in jobs:
            jobs.append("smoke")
        smoke_floor.append("jruby")
        reasons.append("ci:jruby: compare-jruby + smoke jruby (required)")
    if spinel:
        for job in (*CORE, "spinel-framework", "build-site", "archive-results"):
            if job not in jobs:
                jobs.append(job)
        reasons.append("ci:spinel: CORE Spinel lane (required)")
    if not reasons:
        reasons.append("ci focus: BASE only")
    return finish(
        jobs,
        extra,
        smoke_floor,
        False,
        False,
        bool(spinel),
        reasons,
        smoke_extra=extra,
        compare=compare,
        focus_required=bool(extra) or jruby,
        hard_spinel=bool(spinel),
        spinel_tests=list(SPINEL_TESTS) if spinel else None,
    )


def native_coverage(path):
    """Identify native core/focused suites and interpreter-only exceptions."""
    interpreter_only = path.startswith(
        "runtime/spinel/scaffold/ruby_overlay/"
    ) or path in {
        "runtime/spinel/db_jruby.rb",
        "runtime/spinel/markly_jruby.rb",
        "runtime/spinel/db_cruby.rb",
        "runtime/spinel/pg_driver_cruby.rb",
        "runtime/spinel/message_digest_cruby.rb",
        "runtime/spinel/module_delegate.rb",
    }
    native = (
        path.startswith(("runtime/ruby/", "runtime/spinel/", "runtime/gems/", "src/emit/ruby/"))
        or path == "src/emit/ruby.rs"
        or (path.startswith("tests/spinel") and path.endswith((".rs", ".rb")))
        or path in {f"tests/{name}.rs" for name in SPINEL_TESTS}
        or path == "tests/support/db_concurrency_spinel.rb"
    ) and not interpreter_only
    suites = set()
    if path.startswith("runtime/ruby/") and path.endswith((".rb", ".rbs")):
        suites.add("framework_tests_spinel")
    # `Model.transaction` (rollback, nesting, savepoints) lives here; its
    # gate compiles it with the real_blog fixture.
    if path in {
        "runtime/ruby/active_record/connection.rb",
        "runtime/ruby/active_record/connection.rbs",
    }:
        suites.add("spinel_transaction_connection")
    focused = re.fullmatch(r"tests/([^/]+)\.(?:rs|rb)", path)
    if focused and focused[1] in SPINEL_TESTS:
        suites.add(focused[1])
    if path == "tests/support/db_concurrency_spinel.rb":
        suites.add("db_sqlite_concurrency")
    # The fixture app beside its harness selects the same suite.
    if path.startswith("tests/find_or_create_bang_records/"):
        suites.add("find_or_create_bang_records")
    if (
        path in GENERATED_COLUMNS_SPINEL_INPUTS
        or path.startswith("tests/support/generated_columns_")
        or path.startswith("src/lower/arel/")
        or path.startswith("src/lower/model_to_library/adapter_emit/")
    ):
        suites.add("generated_columns_spinel")
    if path in JSON_TYPES_SPINEL_INPUTS:
        suites.add("postgres_json_types_spinel")
    if path in APP_REOPENS_INPUTS:
        suites.add("app_reopens_runtime_class")
    if path in ARRAY_COLUMN_SPINEL_INPUTS or path.startswith("src/lower/model_to_library/adapter_emit/"):
        suites.add("array_column_storage_spinel")
    # Gate drivers stay flat beside their Rust harness. Match the most
    # specific suite first (e.g. param_binds_values before param_binds).
    if path == "tests/param_binds_text_cleanup.rb":
        suites.add("param_binds_cleanup")
    elif path.startswith("tests/") and path.endswith(".rb"):
        stem = path[len("tests/"):-len(".rb")]
        for suite in reversed(PARAM_BIND_TESTS):
            if stem == suite or stem.startswith(suite + "_"):
                suites.add(suite)
                break
    if path == "runtime/spinel/test/statement_cache_cases.rb":
        suites.add("param_binds")
    if path.startswith("runtime/gems/rack_mini_profiler/") or path in {
        "src/gems/rack_mini_profiler.rs",
        "runtime/spinel/request_hooks.rb",
    }:
        suites.add("spinel_mini_profiler")
    if path in {
        "runtime/spinel/active_support_instrumentation.rb",
        "runtime/ruby/active_support_notifications.rb",
        "runtime/ruby/active_support_notifications.rbs",
        "src/lower/view_instrumentation.rs",
    }:
        suites.update(("spinel_active_support_instrumentation", "spinel_mini_profiler"))
    if path in PG_DB_INPUTS:
        suites.add("spinel_pg_db")
    suites.update(DB_CONTRACT_INPUTS.get(path, ()))
    if path in {
        "tests/support/emit_and_run.rs",
        "src/lower/model_to_library/adapter_emit.rs",
        "src/emit/ruby/library.rs",
    } or path.startswith(("src/lower/arel/", "src/lower/model_to_library/adapter_emit/")):
        suites.update(PARAM_BIND_TESTS)
    if path.startswith(("runtime/spinel/", "runtime/ruby/")) and not interpreter_only:
        name = path.rsplit("/", 1)[-1]
        owned_tests = set()
        if path in {
            "runtime/spinel/secure_random_stub.rb",
            "runtime/spinel/secure_random_stub.rbs",
        }:
            owned_tests.add("openssl_random")
        if name == "statement_cache_cases.rb":
            owned_tests.add("param_binds")
        if any(word in name for word in ("web_push", "base64")):
            owned_tests.add("spinel_web_push_crypto")
        if any(
            word in name
            for word in (
                "signed_cookie",
                "message_verifier",
                "signed_id",
                "message_digest",
                "base64",
            )
        ) or path.startswith("runtime/spinel/tep/url."):
            owned_tests.add("rails_compat_vectors_spinel")
        if name in {
            "db_pg.rb",
            "db_pg.rbs",
            "pg_errors.rb",
            "pg_errors.rbs",
            "pg_driver.rb",
            "pg_driver.rbs",
        }:
            # PostgreSQL, not SQLite: the SQLite database suites below
            # never load it.
            owned_tests.add("spinel_pg_db")
        elif any(
            word in path for word in ("/db", "sqlite", "active_support_time_parsing")
        ):
            # Shared database inputs own lease/ownership, binds, cache recency,
            # and the snapshot / write-permit / checkpoint policy.
            owned_tests.update(
                (
                    "spinel_db_lease",
                    *PARAM_BIND_TESTS,
                    "spinel_stmt_cache_lru",
                    "stmt_stats",
                    "db_sqlite_concurrency",
                    "spinel_db_contract",
                )
            )
        if any(word in name for word in ("param", "multipart", "request")):
            owned_tests.add("spinel_param_builder")
        if path in NET_HTTP_INPUTS:
            owned_tests.add("spinel_net_http_start")
        if name in {
            "date.rb",
            "date.rbs",
            "active_support_date_parsing.rb",
            "active_support_date_parsing.rbs",
            "active_record_date_serialization.rb",
            "active_record_date_serialization.rbs",
            "sqlite_adapter.rb",
        }:
            owned_tests.add("date_columns_spinel")
        if name in {
            "active_record_date_serialization.rb",
            "active_record_serialization.rb",
        }:
            owned_tests.add("framework_tests_spinel")
        if (
            path.startswith("runtime/spinel/")
            and not path.startswith("runtime/spinel/scaffold/")
            and not owned_tests
        ):
            owned_tests.add("framework_tests_spinel")
        suites.update(owned_tests)
    if path in ACTIVE_JOB_PAYLOAD_INPUTS or path.startswith("tests/active_job_payload/"):
        suites.add("active_job_payload")
    if path in SIDEKIQ_PUSH_INPUTS or path.startswith("tests/sidekiq_push/"):
        suites.add("sidekiq_push")
    if path in SIDEKIQ_WORKER_INPUTS or path.startswith("tests/sidekiq_worker/"):
        suites.add("sidekiq_worker")
    if (
        path.startswith(("tests/rails_compat/", "tests/params_vectors/"))
        or path == "tests/rails_compat_vectors.rb"
    ):
        suites.add(
            "spinel_param_builder"
            if path.startswith("tests/params_vectors/")
            else "rails_compat_vectors_spinel"
        )
    return native, interpreter_only, suites


def archive_and_campfire_jobs(path, interpreter_only):
    """Select packaging and Campfire consumers, not every native runtime edit."""
    jobs = set()
    if path.startswith("runtime/spinel/scaffold/") and not interpreter_only:
        jobs.update((*CORE, "spinel-smoke", "build-site"))
    if path.startswith(("scripts/campfire-compare", "scripts/build-campfire-compare")):
        jobs.update(
            ("spinel-build", "campfire-spinel-build", "campfire-spinel-compare")
        )
    if path.startswith("scripts/campfire-db-differential"):
        jobs.update(("spinel-build", "campfire-spinel-db"))
    campfire_archive = (
        path.startswith(
            (
                "scripts/build-campfire-archive",
                "scripts/campfire-archive",
                "e2e/campfire/",
            )
        )
        or path == "scripts/campfire-docker-files"
    )
    shared_smoke = (
        path.startswith("e2e/") and not path.startswith("e2e/campfire/")
    ) or path in {
        "scripts/smoke",
        "scripts/ci-playwright-install",
        "scripts/ci-npm",
    }
    if campfire_archive or shared_smoke:
        jobs.update(
            (
                "spinel-build",
                "campfire-archive-build",
                "campfire-smoke",
                "campfire-smoke-docker",
            )
        )
    if shared_smoke:
        jobs.add("spinel-smoke")
    return jobs


def select(
    paths,
    *,
    full=False,
    spinel_lane=False,
    focus_extras=(),
    focus_jruby=False,
    focus_spinel=False,
    focus_compare=(),
    publish=False,
    project_scope=None,
):
    focus_extras = tuple(focus_extras or ())
    focus_compare = tuple(focus_compare or ())
    # Publication always requires full mode — reject before any narrow lane
    # (focus / main Spinel) can silently drop publish=True.
    if publish and not full:
        raise ValueError("publication requires full validation mode")
    # Focus labels narrow the plan before path ownership or unknown-input
    # Spinel. ci:full still falls through to the full ledger below.
    if not full and (focus_extras or focus_jruby or focus_spinel or focus_compare):
        return focus_plan(focus_extras, focus_jruby, focus_spinel, focus_compare)
    if spinel_lane and not full:
        return finish(
            list(SPINEL_LANE),
            [],
            [],
            False,
            False,
            True,
            ["main/unknown: Ruby floor plus advisory Spinel suite"],
            spinel_tests=list(SPINEL_TESTS),
        )
    targets, smoke = set(), set()
    jobs_selected, spinel_tests = set(), set()
    wasm = site = spinel = False
    reasons = []
    for path in paths:
        if path == "src/project.rs" and project_scope in PROJECT_BUILDERS.values():
            targets.update(("ruby", "jruby"))
            smoke.update(("ruby", "jruby"))
            if project_scope == "ruby-family":
                spinel = True
                jobs_selected.update(SPINEL)
                spinel_tests.update(SPINEL_TESTS)
            reasons.append(f"{path}: proven {project_scope} assembly bodies only")
            continue
        match = re.match(r"(?:src/emit/|runtime/)([^/.]+)(?:[/.]|$)", path)
        test = re.match(
            r"tests/(?:framework_tests_)?([a-z]+)_toolchain\.rs$|tests/framework_tests_([a-z]+)\.rs$",
            path,
        )
        target = (
            match[1]
            if match
            else next((v for v in test.groups() if v), None)
            if test
            else None
        )
        if path in JRUBY_BIND_INPUTS:
            targets.add("jruby")
            reasons.append(f"{path}: JDBC bind contract")
        native, interpreter_only, owned_tests = native_coverage(path)
        spinel_tests.update(owned_tests)
        if native or owned_tests:
            spinel = True
            jobs_selected.update(CORE)
        if native:
            reasons.append(f"{path}: native Spinel core")
        if path in SIDEKIQ_LIVE_INPUTS or path.startswith(("tests/sidekiq_push_live/", "tests/sidekiq_worker_live/", "tests/redis_cable_live/")):
            spinel = True
            jobs_selected.add("sidekiq-live")
            reasons.append(f"{path}: live Sidekiq gate")
        if target in TARGETS or target == "spinel":
            owners = (
                {"ruby", "jruby", "spinel"}
                if target in {"ruby", "spinel"} and not path.startswith("runtime/ruby/")
                else {target}
            )
            if (
                path.startswith(("runtime/ruby/", "runtime/spinel/"))
                or (path.startswith("tests/spinel") and path.endswith(".rs"))
                or path in {f"tests/{name}.rs" for name in SPINEL_TESTS}
            ):
                owners = set()  # Native framework coverage; no interpreted archives.
            if interpreter_only:
                owners = (
                    {"jruby"}
                    if path.endswith(("db_jruby.rb", "markly_jruby.rb"))
                    else {"ruby", "jruby"}
                )
            targets.update(owners - {"spinel"})
            smoke.update(owners - {"spinel"})
            spinel |= "spinel" in owners
            if owners:
                reasons.append(f"{path}: {', '.join(sorted(owners))}")
        elif target == "shared":
            full = True
            reasons.append(f"{path}: shared code generation")
        if path.startswith("wasm/"):
            wasm = True
            reasons.append(f"{path}: WASM/browser compiler")
        if path.startswith(("site/", "docs/guide/")):
            site = wasm = True
        if (
            path.startswith("e2e/") and not path.startswith("e2e/campfire/")
        ) or path in {
            "scripts/smoke",
            "scripts/ci-playwright-install",
            "scripts/ci-npm",
            "scripts/create-blog",
            "scripts/create-store",
            "bin/rh",
        }:
            smoke.update(TARGETS)
            spinel = True
        if (
            path.startswith(("tools/compare/", "tests/framework_test_support"))
            or path == "scripts/compare"
        ):
            targets.update(TARGETS)
            spinel = True
            jobs_selected.update(CORE)
            if path.startswith("tests/framework_test_support"):
                spinel_tests.add("framework_tests_spinel")
        archive_jobs = archive_and_campfire_jobs(path, interpreter_only)
        if archive_jobs:
            spinel = True
            jobs_selected.update(archive_jobs)
    if full:
        targets.update(TARGETS)
        smoke.update(TARGETS)
        wasm = site = spinel = True
        reasons.append("full validation requested")
        jobs_selected.update(SPINEL)
        jobs_selected.update(SPINEL_FULL_OR_PATH)
        spinel_tests.update(SPINEL_TESTS)
    if spinel:
        jobs_selected.add("spinel-build")
    jobs = list(BASE)
    extra = [t for t in EXTRA_COMPARE_TARGETS if t in targets]
    compare = [t for t in COMPARE_TARGETS if t in targets]
    floor_smoke = [t for t in FLOOR_SMOKE_TARGETS if t in smoke]
    extra_smoke = [t for t in EXTRA_COMPARE_TARGETS if t in smoke]
    if compare:
        jobs.append("compare")
    if extra:
        jobs.append("compare-extra")
    if "jruby" in targets:
        jobs.append("compare-jruby")
    if wasm or "typescript" in targets:
        jobs.append("browser-smoke-typescript")
    if wasm:
        jobs.extend(["build-wasm", "browser-smoke-ide"])
    if floor_smoke or extra_smoke or site:
        jobs.append("build-site")
    if floor_smoke:
        jobs.append("smoke")
    if extra_smoke:
        jobs.append("smoke-extra")
    if spinel_tests:
        jobs_selected.add("spinel-framework")
    if "spinel-smoke" in jobs_selected:
        jobs_selected.add("build-site")
    if "build-site" in jobs_selected and "build-site" not in jobs:
        jobs.append("build-site")
    if "build-site" in jobs or {"build-site", "campfire-archive-build"} & jobs_selected:
        jobs_selected.add("archive-results")
    jobs.extend(
        j
        for j in [*SPINEL, *SPINEL_FULL_OR_PATH, "archive-results"]
        if j in jobs_selected
    )
    if publish:
        if not full:
            raise ValueError("publication requires full validation mode")
        jobs.append("assemble-site")
    return finish(
        jobs,
        extra,
        floor_smoke,
        wasm,
        site,
        spinel,
        reasons,
        publish,
        [t for t in SPINEL_TESTS if t in spinel_tests],
        smoke_extra=extra_smoke,
        compare=compare,
    )


def positive_seconds(value):
    # A bound, not math.isfinite: a huge JSON integer overflows its float conversion; NaN and inf fail it too.
    return isinstance(value, (int, float)) and not isinstance(value, bool) and 0 < value < 86400


def load_spinel_timings(raw):
    """Recorded seconds per kind and test; anything malformed counts as unrecorded."""
    try:
        data = json.loads(raw) if raw.strip() else {}
    except ValueError:
        return {}
    if not isinstance(data, dict):
        return {}
    return {
        kind: {
            name: seconds
            for name, seconds in data[kind].items()
            if positive_seconds(seconds)
        }
        for kind in SPINEL_TIMING_KINDS
        if isinstance(data.get(kind), dict)
    }


def spinel_timings_from_env():
    path = os.environ.get("CI_SPINEL_TIMINGS_FILE", "")
    try:
        raw = Path(path).read_text() if path else os.environ.get("CI_SPINEL_TIMINGS", "")
    except (OSError, UnicodeError):
        raw = ""
    return load_spinel_timings(raw)


def spinel_test_seconds(test, timings=None):
    recorded = (timings or {}).get("framework", {}).get(test)
    return recorded or SPINEL_TEST_SECONDS.get(test, SPINEL_TEST_DEFAULT_SECONDS)


def spinel_toolchain_seconds(test, timings=None):
    recorded = (timings or {}).get("toolchain", {}).get(test)
    return recorded or SPINEL_TOOLCHAIN_SECONDS.get(test, SPINEL_TOOLCHAIN_DEFAULT_SECONDS)


def spinel_framework_shards(tests, count=SPINEL_FRAMEWORK_SHARDS, timings=None):
    """Longest suite first onto the lightest shard; each shard keeps SPINEL_TESTS order."""
    shards = [[] for _ in range(min(count, len(tests)) or 1)]
    loads = [0] * len(shards)
    for test in sorted(tests, key=lambda t: -spinel_test_seconds(t, timings)):
        i = loads.index(min(loads))
        shards[i].append(test)
        loads[i] += spinel_test_seconds(test, timings)
    return [{"shard": i, "tests": [t for t in tests if t in shard]} for i, shard in enumerate(shards)]


def spinel_toolchain_shard(tests, shard, count=SPINEL_TOOLCHAIN_SHARDS, timings=None):
    """This shard's tests: longest first onto the lightest shard, by name order within it."""
    loads = [0] * count
    tail = (timings or {}).get("toolchain", {}).get(SPINEL_TOOLCHAIN_TAIL, SPINEL_TOOLCHAIN_TAIL_SECONDS)
    loads[-1] = tail * SPINEL_TOOLCHAIN_THREADS
    mine = []
    for test in sorted(tests, key=lambda t: (-spinel_toolchain_seconds(t, timings), t)):
        i = loads.index(min(loads))
        loads[i] += spinel_toolchain_seconds(test, timings)
        if i == shard:
            mine.append(test)
    return sorted(mine)


def record_timing(kind, name, seconds, ok):
    if out := os.environ.get("CI_SPINEL_TIMINGS_OUT"):
        Path(out).parent.mkdir(parents=True, exist_ok=True)
        with open(out, "a") as f:
            f.write(json.dumps({"kind": kind, "name": name, "seconds": max(round(seconds), 1), "ok": ok}) + "\n")


def run_timed(kind, name, command):
    start = time.monotonic()
    status = subprocess.run(command).returncode
    record_timing(kind, name, time.monotonic() - start, status == 0)
    return status


def run_tests(kind, tests, parallel, command):
    """Each test in its own process, so each one's seconds can be recorded."""
    with ThreadPoolExecutor(max_workers=max(parallel, 1)) as pool:
        statuses = list(pool.map(lambda t: run_timed(kind, t, [*command, t]), tests))
    failed = [t for t, status in zip(tests, statuses) if status != 0]
    print(f"{len(tests) - len(failed)} passed, {len(failed)} failed")
    for test in failed:
        print(f"::error::{test} failed")
    return bool(failed)


def merge_timings(previous, records, weight=0.5):
    """Each passing test's seconds averaged into its last value; failures keep the last value."""
    merged = {kind: dict(previous.get(kind, {})) for kind in SPINEL_TIMING_KINDS}
    for record in records:
        kind, name, seconds = record.get("kind"), record.get("name"), record.get("seconds")
        if not isinstance(kind, str) or kind not in merged or not isinstance(name, str):
            continue
        if record.get("ok") is not True or not positive_seconds(seconds):
            continue
        last = merged[kind].get(name)
        merged[kind][name] = round(last + (seconds - last) * weight) if last else round(seconds)
    return merged


def finish(
    jobs,
    extra,
    smoke,
    wasm,
    site,
    spinel,
    reasons,
    publish=False,
    spinel_tests=None,
    *,
    smoke_extra=None,
    compare=None,
    focus_required=False,
    hard_spinel=False,
):
    smoke_extra = list(smoke_extra or [])
    compare = list(compare or [])
    archives = (
        ["blog", "spinel", *TARGETS, "typescript-worker", "futamura"]
        if site
        else [
            *smoke,
            *smoke_extra,
            *(["spinel"] if "spinel-smoke" in jobs else []),
        ]
    )
    # ci:spinel focus selects build-site / archive-results without spinel-smoke
    # (Campfire Spinel suite stays off). Still emit the spinel browse archive so
    # `roundhouse --archives` is never called with an empty list (#592).
    if hard_spinel and "build-site" in jobs and "spinel" not in archives:
        archives = [*archives, "spinel"]
    advisory = set(ADVISORY)
    if hard_spinel:
        # Focused CORE Spinel is merge-gate required for the fix round.
        advisory -= SPINEL_FOCUS_HARD
    if not focus_required:
        advisory |= LEDGER_EXTRAS & set(jobs)
    extras_advisory = bool(LEDGER_EXTRAS & advisory & set(jobs))
    # Workflow CoE for CORE / spinel-framework follows this flag.
    spinel_advisory = bool(SPINEL_FOCUS_HARD & advisory & set(jobs))
    return {
        "jobs": jobs,
        "required": [j for j in jobs if j not in advisory and j not in REPORTING],
        "advisory": sorted(advisory & set(jobs)),
        "extra_compare": extra,
        "compare": compare,
        "smoke": smoke,
        "smoke_extra": smoke_extra,
        "archives": archives,
        "wasm": wasm,
        "site": site,
        "spinel": spinel,
        "spinel_tests": spinel_tests or [],
        "spinel_framework_shards": spinel_framework_shards(spinel_tests or []),
        "publish": publish,
        "extras_advisory": extras_advisory,
        "spinel_advisory": spinel_advisory,
        "reasons": reasons,
    }


def git(*args):
    return subprocess.check_output(["git", *args])


def ensure_commit(sha):
    """Fetch a commit by SHA when the plan checkout is too shallow to see it."""
    try:
        git("cat-file", "-e", f"{sha}^{{commit}}")
    except subprocess.CalledProcessError:
        git("fetch", "--no-tags", "--depth=1", "origin", sha)
        git("cat-file", "-e", f"{sha}^{{commit}}")


def project_change_scope(before, after):
    """Narrow only body-only edits in known builders; all other bytes must match.

    This is not a Rust parser. Only indented bodies without raw strings or
    block comments qualify; unknown shapes/signatures/items do not narrow
    and stay on the Ruby floor.
    """
    pattern = re.compile(
        r"(?P<header>^fn (?P<name>"
        + "|".join(sorted(PROJECT_BUILDERS))
        + r")\([^{};]*\{\n)(?P<body>(?:[ \t]+[^\n]*\n|\n)*)^}\n",
        re.MULTILINE,
    )
    # Exclude function-looking text in Rust literals/comments. Block comments
    # (including nested ones) are deliberately unsupported, not half-parsed.
    literals = re.compile(
        r'//[^\n]*|\b[bc]?r(?P<hash>#+)"[\s\S]*?"(?P=hash)'
        r'|"(?:\\[\s\S]|[^"\\])*"'
        r"|'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'"
        r"|/\*"
    )
    bodies = []
    skeletons = []
    for source in (before, after):
        found = {}
        excluded = list(literals.finditer(source))
        if any(token[0] == "/*" for token in excluded):
            return None
        lexical = literals.sub(lambda token: re.sub(r"[^\n]", " ", token[0]), source)

        def mask(match):
            if any(token.start() <= match.start() < token.end() for token in excluded):
                return match[0]
            prefix = lexical[: match.start()]
            if any(
                prefix.count(left) != prefix.count(right)
                for left, right in [("{", "}"), ("(", ")"), ("[", "]")]
            ):
                return match[0]  # Nested/macro input is not a top-level builder.
            name, body = match["name"], match["body"]
            code = "\n".join(
                line for line in body.splitlines() if not line.lstrip().startswith("//")
            )
            if name in found or re.search(r'(?<!\w)[bc]?r#*"|/\*', code):
                raise ValueError("ambiguous project assembly body")
            found[name] = body
            return match["header"] + "}\n"

        try:
            skeletons.append(pattern.sub(mask, source))
        except ValueError:
            return None
        bodies.append(found)
    if skeletons[0] != skeletons[1] or bodies[0].keys() != bodies[1].keys():
        return None
    changed = {name for name in bodies[0] if bodies[0][name] != bodies[1][name]}
    if changed:
        scopes = {PROJECT_BUILDERS[name] for name in changed}
        return "ruby-family" if "ruby-family" in scopes else "interpreted"
    return None


def changed_inputs(event, event_name, sha, *, need_project_scope=True):
    if not SHA.fullmatch(sha) or git("rev-parse", "HEAD").decode().strip() != sha:
        raise ValueError("checkout is not the event SHA")
    if event_name == "pull_request":
        pr = event["pull_request"]
        base, head = pr["base"]["sha"], pr["head"]["sha"]
        if not SHA.fullmatch(base) or not SHA.fullmatch(head):
            raise ValueError("PR event is missing base/head SHAs")
        parents = git("show", "-s", "--format=%P", "HEAD").decode().split()
        # Prefer the merge commit's first parent when this is the PR merge
        # tree: GitHub's merge ref can land on a newer main than the event's
        # base.sha. Do not fetch the fork head from origin; it is already a
        # parent of the merge commit, or HEAD itself.
        if len(parents) == 2 and parents[1] == head:
            base = parents[0]
        elif sha != head:
            raise ValueError("checkout is not the event's PR merge tree or head")
        ensure_commit(base)
    elif event_name == "push":
        base = event["before"]
        if not SHA.fullmatch(base) or base == "0" * 40:
            raise ValueError("no previous main tree")
        ensure_commit(base)
    else:
        return [], None
    # Renames become a deletion and addition; both ownership sets are selected.
    paths = [
        p.decode("utf-8")
        for p in git("diff", "--name-only", "--no-renames", "-z", base, sha).split(
            b"\0"
        )
        if p
    ]
    scope = None
    if need_project_scope and "src/project.rs" in paths:
        entries = [
            git("ls-tree", ref, "--", "src/project.rs").split() for ref in (base, sha)
        ]
        if all(entry and entry[0] == b"100644" for entry in entries):
            scope = project_change_scope(
                git("show", f"{base}:src/project.rs").decode("utf-8"),
                git("show", f"{sha}:src/project.rs").decode("utf-8"),
            )
    return paths, scope


def check_results(plan, needs, *, compact=False):
    if compact:
        # Compact gate only observes the publication floor jobs in its needs
        # graph. Spinel/full extras are enforced by ci-summary, not here.
        required = [job for job in PUBLICATION if job in plan["jobs"]]
    else:
        required = plan["required"]
    failures = [
        f"{j}: {needs.get(j, {}).get('result', 'missing')}"
        for j in required
        if needs.get(j, {}).get("result") != "success"
    ]
    if needs.get("plan", {}).get("result") != "success":
        failures.append("plan: no successful routing decision")
    if (
        not compact
        and needs.get("compact-required", {}).get("result") != "success"
    ):
        failures.append("compact-required: no successful baseline gate")
    # Advisory work never blocks the gate, but incomplete work is not complete.
    # Compact only claims completeness for the publication floor it can see.
    # Prefer the plan's advisory set (focus can harden CORE Spinel / extras);
    # older plan blobs without the field fall back to the static Spinel set.
    advisory = set(plan["advisory"]) if "advisory" in plan else set(ADVISORY)
    tracked = required if compact else plan["jobs"]
    complete = not failures and all(
        _advisory_complete(j, needs.get(j, {}), advisory, plan) for j in tracked
    )
    return failures, complete


def _advisory_complete(job, need, advisory, plan=None):
    """Whether one selected job counts as actually finished.

    Under continue-on-error, GitHub reports needs.*.result=success even when
    the job failed. Spinel advisory jobs expose an execution (or GC-mode)
    output from an always() receipt; require that. Matrix ledger extras
    (compare-extra / smoke-extra) cannot prove every cell passed via a
    single job output, so they never count as complete while advisory —
    merge still ignores them via plan["required"].
    """
    if need.get("result") != "success":
        return False
    if job not in advisory:
        return True
    if job in LEDGER_EXTRAS:
        return False
    keys = {
        "campfire-spinel-compare": ["default", "minor-gc", "verify-gen"],
        "spinel-toolchain": [f"shard-{i}" for i in range(SPINEL_TOOLCHAIN_SHARDS)],
        "spinel-framework": [
            f"shard-{s['shard']}"
            for s in (plan or {}).get("spinel_framework_shards", [])
        ]
        or ["execution"],
    }.get(job, ["execution"])
    return all(need.get("outputs", {}).get(key) == "success" for key in keys)


def write_outputs(values):
    output = os.environ.get("GITHUB_OUTPUT")
    lines = "".join(
        f"{key}={json.dumps(value, separators=(',', ':')) if not isinstance(value, str) else value}\n"
        for key, value in values.items()
    )
    if output:
        with open(output, "a") as f:
            f.write(lines)
    print(lines, end="")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=["plan", "gate", "compact-gate", "toolchain-shard", "timed", "run-tests", "merge-timings"],
    )
    parser.add_argument("--shard", type=int)
    parser.add_argument("--kind", choices=SPINEL_TIMING_KINDS)
    parser.add_argument("--name")
    parser.add_argument("--parallel", type=int, default=os.cpu_count() or 1)
    parser.add_argument("--previous")
    parser.add_argument("--records")
    parser.add_argument("--out")
    argv = sys.argv[1:]
    split = argv.index("--") if "--" in argv else len(argv)
    args = parser.parse_args(argv[:split])
    command = argv[split + 1 :]
    if args.command == "toolchain-shard":
        tests = [line.strip() for line in sys.stdin if line.strip()]
        for test in spinel_toolchain_shard(tests, args.shard, timings=spinel_timings_from_env()):
            print(test)
        return False
    if args.command == "timed":
        return run_timed(args.kind, args.name, command) != 0
    if args.command == "run-tests":
        tests = [line.strip() for line in sys.stdin if line.strip()]
        return run_tests(args.kind, tests, args.parallel, command)
    if args.command == "merge-timings":
        try:
            previous = load_spinel_timings(Path(args.previous).read_text())
        except (OSError, UnicodeError):
            previous = {}
        records = []
        for path in sorted(Path(args.records).glob("*.jsonl")) if Path(args.records).is_dir() else []:
            for line in path.read_text().splitlines():
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if isinstance(record, dict):
                    records.append(record)
        merged = merge_timings(previous, records)
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(json.dumps(merged, indent=1, sort_keys=True) + "\n")
        print(f"merged {len(records)} records: " + ", ".join(f"{k} {len(v)}" for k, v in merged.items()))
        return False
    if args.command != "plan":
        raw_plan = os.environ.get("CI_PLAN", "")
        if not raw_plan.strip():
            # Plan cancelled/skipped leaves an empty output; do not crash the
            # gates or claim a green floor.
            write_outputs({"complete": False})
            print("::notice::No plan output (cancelled or skipped); incomplete")
            return True
        plan = json.loads(raw_plan)
        failures, complete = check_results(
            plan,
            json.loads(os.environ["CI_NEEDS"]),
            compact=args.command == "compact-gate",
        )
        write_outputs({"complete": complete})
        for failure in failures:
            print(f"::error::{failure}")
        return bool(failures)
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    event_name = os.environ["GITHUB_EVENT_NAME"]
    pr = event.get("pull_request", {})
    coverage = parse_coverage_labels(
        {label["name"] for label in pr.get("labels", [])},
        env_full=os.environ.get("CI_FULL") == "true",
    )
    marker = full_marker_commit(event, event_name, os.environ.get("GITHUB_REF"))
    full = coverage.full or marker is not None
    focus_extras = coverage.focus_extras
    focus_jruby = coverage.focus_jruby
    focus_spinel = coverage.focus_spinel
    focus_compare = coverage.focus_compare
    any_focus = bool(focus_extras or focus_jruby or focus_spinel or focus_compare)
    # Advisory full Spinel suite for unknown changed inputs only (not every
    # main push). Ordinary main/PR runs use path ownership; Full / ci:full /
    # [ci full] still select the broad suite. PR `ci:spinel` is focus_plan.
    spinel_lane = False
    reason = None
    try:
        # project_scope only narrows path selection; full / focus / spinel
        # short-circuit before that, so skip the expensive project.rs body
        # scan there.
        paths, project_scope = changed_inputs(
            event,
            event_name,
            os.environ["GITHUB_SHA"],
            need_project_scope=not full and not any_focus and not spinel_lane,
        )
    except (KeyError, ValueError, UnicodeError, subprocess.CalledProcessError) as e:
        paths, project_scope = [], None
        if full:
            reason = f"Unknown changed inputs: {e}; running full validation"
        elif any_focus:
            reason = (
                f"Unknown changed inputs: {e}; "
                "focus labels keep BASE+selected lanes"
            )
        else:
            spinel_lane = True
            reason = (
                f"Unknown changed inputs: {e}; "
                "Ruby+Spinel only (extra-language SDKs not selected)"
            )
    publish = os.environ.get("CI_PUBLISH") == "true"
    if publish and (
        os.environ["GITHUB_REPOSITORY"] != "rubys/roundhouse"
        or os.environ["GITHUB_REF"] != "refs/heads/main"
        or event_name not in {"schedule", "workflow_dispatch"}
    ):
        raise ValueError("publication is only allowed by canonical main's full caller")
    plan = select(
        paths,
        full=full,
        spinel_lane=spinel_lane,
        focus_extras=focus_extras,
        focus_jruby=focus_jruby,
        focus_spinel=focus_spinel,
        focus_compare=focus_compare,
        publish=publish,
        project_scope=project_scope,
    )
    if reason:
        plan["reasons"].append(reason)
    if marker is not None and not coverage.full:
        plan["reasons"].append(f"{CI_FULL_MARKER} in commit {marker[:12]}".rstrip())
    timings = spinel_timings_from_env()
    plan["spinel_framework_shards"] = spinel_framework_shards(plan["spinel_tests"] or [], timings=timings)
    if not any(timings.values()):
        plan["reasons"].append("No recorded Spinel test seconds: shards balanced by the static tables")
    spinel = os.environ.get("CI_SPINEL_REVISION", "")
    if plan["spinel"] and not spinel:
        try:
            spinel = subprocess.check_output(
                ["gh", "api", "repos/matz/spinel/commits/master", "--jq", ".sha"],
                text=True,
            ).strip()
        except subprocess.CalledProcessError:
            spinel = "master"
            plan["reasons"].append(
                "Spinel lookup unavailable: fresh master build; actual revision recorded by producer"
            )
    if spinel and spinel != "master" and not SHA.fullmatch(spinel):
        raise ValueError("invalid Spinel revision")
    write_outputs(
        {
            "plan": plan,
            "jobs": plan["jobs"],
            "extra-compare": plan["extra_compare"],
            "compare": plan["compare"],
            "spinel-tests": plan["spinel_tests"],
            "spinel-framework-shards": plan["spinel_framework_shards"],
            "spinel-timings": {"toolchain": timings.get("toolchain", {})},
            "smoke": plan["smoke"],
            "smoke-extra": plan["smoke_extra"],
            "extras-advisory": plan["extras_advisory"],
            "spinel-advisory": plan["spinel_advisory"],
            "archives": ",".join(plan["archives"]),
            "wasm": plan["wasm"],
            "site": plan["site"],
            "publish": plan["publish"],
            "spinel-revision": spinel,
        }
    )
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        Path(summary).write_text(
            "## Selected CI coverage\n\n```json\n"
            + json.dumps(plan, indent=2)
            + "\n```\n"
        )
    return False


if __name__ == "__main__":
    raise SystemExit(main())
