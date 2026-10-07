"""Routing boundaries and false-green checks, without GitHub or toolchains."""

import importlib.util
import json
import os
import re
import subprocess
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "ci_plan", Path(__file__).parents[1] / "scripts/ci-plan.py"
)
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


@contextmanager
def git_repository():
    env = os.environ.copy()
    for name in (
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
    ):
        env.pop(name, None)
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)

        def git(*args):
            return (
                subprocess.check_output(
                    ["git", "-C", directory, *args],
                    stderr=subprocess.DEVNULL,
                    env=env,
                )
                .decode()
                .strip()
            )

        git("init")
        git("config", "user.name", "CI test")
        git("config", "user.email", "test@example.invalid")
        previous = os.getcwd()
        try:
            os.chdir(root)
            # Direct planner calls and CLI subprocesses also see the clean env.
            with patch.dict(os.environ, env, clear=True):
                yield root, git
        finally:
            os.chdir(previous)


class Routing(unittest.TestCase):
    def extras(self, plan):
        return set(plan["jobs"]) - set(ci.BASE)

    def test_general_analysis_and_lowering_retain_base(self):
        plan = ci.select(["src/analyze/call.rs", "src/lower/rails.rs"])
        self.assertEqual(plan["jobs"], ci.BASE)
        self.assertEqual(plan["archives"], [])
        self.assertNotIn("compare", plan["jobs"])
        self.assertNotIn("browser-smoke-typescript", plan["jobs"])
        self.assertNotIn("compare-extra", plan["jobs"])

    def test_ruby_floor_omits_rust_typescript_until_those_owners_change(self):
        self.assertEqual(
            ci.BASE,
            [
                "generate-fixture",
                "unit",
                "build-roundhouse",
                "store-check",
                "compare-ruby",
                "campfire-conformance",
                "campfire-compare",
            ],
        )
        rust = ci.select(["src/emit/rust.rs"])
        self.assertIn("compare", rust["jobs"])
        self.assertNotIn("browser-smoke-typescript", rust["jobs"])
        typescript = ci.select(["src/emit/typescript.rs"])
        self.assertIn("compare", typescript["jobs"])
        self.assertIn("browser-smoke-typescript", typescript["jobs"])
        wasm = ci.select(["wasm/lib/driver.mjs"])
        self.assertIn("browser-smoke-typescript", wasm["jobs"])
        self.assertNotIn("compare", wasm["jobs"])

    def test_shared_emitters_select_cross_target_full_coverage(self):
        for path in [
            "src/emit/shared/schema_sql.rs",
            "src/emit/shared/ops.rs",
            "src/emit/shared/mod.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(
                    plan["extra_compare"],
                    ["crystal", "kotlin", "swift", "csharp", "go", "elixir", "python"],
                )
                self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
                self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
                self.assertTrue(plan["extras_advisory"])
                self.assertTrue(plan["site"])
                self.assertTrue(plan["wasm"])
                self.assertTrue(set(ci.SPINEL11).issubset(plan["jobs"]))
                self.assertIn("writebook-inventory", plan["required"])
                self.assertIn("archive-results", plan["jobs"])
                self.assertNotIn("archive-results", plan["required"])
                self.assertNotIn("compare-extra", plan["required"])
                self.assertNotIn("smoke-extra", plan["required"])

    def test_native_only_test_selects_core_without_archives_or_campfire(self):
        plan = ci.select(["tests/spinel_toolchain.rs"])
        self.assertEqual(self.extras(plan), set(ci.CORE))
        self.assertEqual(plan["spinel_tests"], [])
        self.assertEqual(plan["archives"], [])
        self.assertFalse(
            any("campfire" in job for job in plan["jobs"] if job not in ci.BASE)
        )

    def test_shared_runtime_rb_and_rbs_select_framework_native_coverage(self):
        for path in ["runtime/ruby/active_record.rb", "runtime/ruby/test/model.rbs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(
                    self.extras(plan), set(ci.CORE) | {"spinel-framework"}
                )
                self.assertEqual(plan["spinel_tests"], ["framework_tests_spinel"])
                self.assertEqual(plan["archives"], [])

    def test_missing_plan_output_fails_gates_without_crashing(self):
        with (
            patch.dict(os.environ, {"CI_PLAN": "", "CI_NEEDS": "{}"}, clear=False),
            patch("sys.argv", ["ci-plan.py", "compact-gate"]),
            patch.object(ci, "write_outputs") as output,
        ):
            self.assertEqual(ci.main(), 1)
        self.assertEqual(output.call_args.args[0]["complete"], False)
        with (
            patch.dict(os.environ, {"CI_PLAN": "", "CI_NEEDS": "{}"}, clear=False),
            patch("sys.argv", ["ci-plan.py", "gate"]),
            patch.object(ci, "write_outputs") as output,
        ):
            self.assertEqual(ci.main(), 1)
        self.assertEqual(output.call_args.args[0]["complete"], False)

    def test_spinel_lane_skips_other_languages(self):
        plan = ci.select(
            ["src/emit/go.rs", "wasm/lib/driver.mjs"],
            spinel_lane=True,
        )
        self.assertEqual(plan["jobs"], ci.SPINEL_LANE)
        self.assertEqual(plan["extra_compare"], [])
        self.assertEqual(plan["smoke"], [])
        self.assertFalse(plan["wasm"])
        self.assertFalse(plan["site"])
        self.assertTrue(plan["spinel"])
        self.assertEqual(plan["spinel_tests"], ci.SPINEL_TESTS)
        self.assertTrue(set(ci.SPINEL11).issubset(plan["jobs"]))
        self.assertNotIn("compare-extra", plan["jobs"])
        self.assertNotIn("compare-jruby", plan["jobs"])
        self.assertNotIn("writebook-inventory", plan["jobs"])
        self.assertNotIn("build-wasm", plan["jobs"])

    def test_spinel_compact_gate_only_requires_publication_floor(self):
        plan = ci.select([], spinel_lane=True)
        needs = {
            "plan": {"result": "success"},
            "generate-fixture": {"result": "success"},
            "unit": {"result": "success"},
            "build-roundhouse": {"result": "success"},
            "store-check": {"result": "success"},
            "compare-ruby": {"result": "success"},
            "campfire-conformance": {"result": "success"},
            "campfire-compare": {"result": "success"},
            "compare": {"result": "skipped"},
            "browser-smoke-typescript": {"result": "skipped"},
        }
        self.assertEqual(ci.check_results(plan, needs, compact=True), ([], True))
        # Non-compact still needs the Spinel required set + compact-required.
        needs["compact-required"] = {"result": "success"}
        for job in plan["required"]:
            needs.setdefault(job, {"result": "success"})
        for job in plan["jobs"]:
            needs.setdefault(job, {"result": "success", "outputs": {"execution": "success"}})
        # Advisory Spinel GC matrix needs per-mode outputs when present.
        if "campfire-spinel-compare" in plan["jobs"]:
            needs["campfire-spinel-compare"] = {
                "result": "success",
                "outputs": {
                    "default": "success",
                    "minor-gc": "success",
                    "verify-gen": "success",
                },
            }
        self.assertEqual(ci.check_results(plan, needs, compact=False)[0], [])

    def test_full_overrides_spinel_without_enabling_publication(self):
        plan = ci.select(["README.md"], spinel_lane=True, full=True)
        self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
        self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
        self.assertTrue(plan["extras_advisory"])
        self.assertTrue(plan["site"])
        self.assertTrue(plan["wasm"])
        self.assertTrue(set(ci.SPINEL11).issubset(plan["jobs"]))
        self.assertIn("build-roundhouse", plan["required"])
        self.assertIn("archive-results", plan["jobs"])
        self.assertNotIn("archive-results", plan["required"])
        self.assertNotIn("assemble-site", plan["jobs"])

    def test_canonical_main_push_selects_spinel_lane_without_extra_sdks(self):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            event.write_text(json.dumps({"before": "0" * 40}))
            env = {
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_EVENT_NAME": "push",
                "GITHUB_SHA": "1" * 40,
                "GITHUB_REF": "refs/heads/main",
                "CI_SPINEL_REVISION": "2" * 40,
            }
            with (
                patch.dict(os.environ, env, clear=True),
                patch("sys.argv", ["ci-plan.py", "plan"]),
                patch.object(
                    ci, "changed_inputs", return_value=(["src/emit/go.rs"], None)
                ),
                patch.object(ci, "write_outputs") as output,
            ):
                self.assertEqual(ci.main(), 0)
            plan = output.call_args.args[0]["plan"]
            self.assertEqual(plan["jobs"], ci.SPINEL_LANE)
            self.assertEqual(plan["extra_compare"], [])
            self.assertEqual(plan["smoke"], [])
            self.assertFalse(plan["wasm"])
            self.assertNotIn("compare-extra", plan["jobs"])
            self.assertNotIn("compare-jruby", plan["jobs"])
            self.assertNotIn("assemble-site", plan["jobs"])
            self.assertTrue(set(ci.SPINEL11).issubset(plan["jobs"]))

    def test_full_input_on_main_still_selects_every_sdk(self):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            event.write_text(json.dumps({"before": "0" * 40}))
            env = {
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_EVENT_NAME": "push",
                "GITHUB_SHA": "1" * 40,
                "GITHUB_REF": "refs/heads/main",
                "CI_FULL": "true",
                "CI_SPINEL_REVISION": "2" * 40,
            }
            with (
                patch.dict(os.environ, env, clear=True),
                patch("sys.argv", ["ci-plan.py", "plan"]),
                patch.object(ci, "changed_inputs", return_value=(["README.md"], None)),
                patch.object(ci, "write_outputs") as output,
            ):
                self.assertEqual(ci.main(), 0)
            plan = output.call_args.args[0]["plan"]
            self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
            self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
            self.assertTrue(plan["extras_advisory"])
            self.assertTrue(plan["wasm"])
            self.assertIn("compare-extra", plan["jobs"])
            self.assertTrue(set(ci.SPINEL11).issubset(plan["jobs"]))

    def test_unknown_changed_inputs_keep_ruby_and_spinel(self):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            event.write_text(json.dumps({"pull_request": {"labels": []}}))
            env = {
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_SHA": "1" * 40,
                "CI_SPINEL_REVISION": "2" * 40,
            }
            with (
                patch.dict(os.environ, env, clear=True),
                patch("sys.argv", ["ci-plan.py", "plan"]),
                patch.object(ci, "changed_inputs", side_effect=ValueError("no tree")),
                patch.object(ci, "write_outputs") as output,
            ):
                self.assertEqual(ci.main(), 0)
            plan = output.call_args.args[0]["plan"]
            self.assertEqual(plan["jobs"], ci.SPINEL_LANE)
            self.assertEqual(plan["extra_compare"], [])
            self.assertFalse(plan["wasm"])
            self.assertTrue(
                any("Unknown changed inputs" in reason for reason in plan["reasons"])
            )
            self.assertTrue(
                any("extra-language SDKs not selected" in reason for reason in plan["reasons"])
            )

    def test_draft_and_ready_events_use_the_same_paths_and_labels(self):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            env = {
                "GITHUB_EVENT_PATH": str(event),
                "GITHUB_EVENT_NAME": "pull_request",
                "GITHUB_SHA": "1" * 40,
                "CI_SPINEL_REVISION": "2" * 40,
            }
            cases = [
                (["README.md"], [], [], [], False),
                (["README.md"], ["ci:draft"], [], [], False),
                (["src/emit/go.rs"], [], [], ["go"], False),
                (["README.md"], ["ci:spinel"], [], [], True),
                (["README.md"], ["ci:swift"], [], ["swift"], False),
                (
                    ["README.md"],
                    ["ci:full", "ci:spinel"],
                    ["rust", "typescript", "ruby", "jruby"],
                    list(ci.EXTRA_COMPARE_TARGETS),
                    True,
                ),
                ([".github/workflows/ci.yml"], [], [], [], False),
            ]
            for paths, labels, expected_smoke, expected_extra, expect_spinel in cases:
                for draft in (False, True):
                    with self.subTest(paths=paths, labels=labels, draft=draft):
                        event.write_text(json.dumps({
                            "pull_request": {
                                "draft": draft,
                                "labels": [{"name": label} for label in labels],
                            }
                        }))
                        with (
                            patch.dict(os.environ, env, clear=True),
                            patch("sys.argv", ["ci-plan.py", "plan"]),
                            patch.object(ci, "changed_inputs", return_value=(paths, None)),
                            patch.object(ci, "write_outputs") as output,
                        ):
                            self.assertEqual(ci.main(), 0)
                        plan = output.call_args.args[0]["plan"]
                        self.assertEqual(plan["smoke"], expected_smoke)
                        self.assertEqual(plan["smoke_extra"], expected_extra)
                        self.assertEqual(plan["spinel"], expect_spinel)
                        self.assertTrue(set(ci.BASE).issubset(plan["required"]))
                        self.assertNotIn("assemble-site", plan["jobs"])
                        if paths == ["README.md"] and labels in ([], ["ci:draft"]):
                            self.assertEqual(plan["jobs"], ci.BASE)
                        elif labels == ["ci:spinel"]:
                            self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))
                            self.assertIn("spinel-compare", plan["required"])
                            self.assertNotIn("campfire-spinel-compare", plan["jobs"])
                        elif labels == ["ci:swift"]:
                            self.assertEqual(plan["extra_compare"], ["swift"])
                            self.assertIn("compare-extra", plan["required"])
                            self.assertIn("smoke-extra", plan["required"])
                            self.assertNotIn("build-wasm", plan["jobs"])

    def test_contract_tests_do_not_expand_the_exercised_workflows(self):
        paths = [
            "tests/ci_plan_test.py",
            "tests/ci_plan_focus_test.py",
            "tests/ci_archive_evidence_test.py",
            "tests/workflow_yaml_parses.rs",
            "tests/ci_policy_workflow.rs",
            "tests/ci_fixture_workflow.rs",
        ]
        for path in paths:
            with self.subTest(path=path):
                self.assertEqual(ci.select([path])["jobs"], ci.BASE)
        self.assertEqual(ci.select(paths)["archives"], [])
        # CI workflow edits stay on the Ruby floor; a real owner still adds its lane.
        self.assertEqual(ci.select(paths + [".github/workflows/ci.yml"])["jobs"], ci.BASE)
        self.assertEqual(ci.select(paths + [".github/workflows/ci.yml"])["smoke"], [])
        partial = ci.select(paths + ["src/emit/go.rs"])
        self.assertEqual(partial["extra_compare"], ["go"])
        self.assertEqual(partial["smoke"], [])
        self.assertEqual(partial["smoke_extra"], ["go"])
        self.assertTrue(partial["extras_advisory"])
        self.assertNotIn("build-wasm", partial["jobs"])
        full = ci.select(paths, full=True)
        self.assertEqual(full["smoke"], ["rust", "typescript", "ruby", "jruby"])
        self.assertEqual(full["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))

    def test_target_partial_does_not_pull_in_wasm_or_other_archives(self):
        plan = ci.select(["src/emit/go/expressions.rs"])
        self.assertEqual(plan["extra_compare"], ["go"])
        self.assertEqual(plan["archives"], ["go"])
        self.assertEqual(plan["smoke"], [])
        self.assertEqual(plan["smoke_extra"], ["go"])
        self.assertTrue(plan["extras_advisory"])
        self.assertFalse(plan["site"])
        self.assertFalse(plan["wasm"])
        self.assertNotIn("spinel-build", plan["jobs"])

    def test_baseline_emitter_adds_its_archive_not_duplicate_compare(self):
        plan = ci.select(["src/emit/rust.rs"])
        self.assertEqual(plan["extra_compare"], [])
        self.assertEqual(plan["compare"], ["rust"])
        self.assertEqual(plan["archives"], ["rust"])
        self.assertNotIn("compare-extra", plan["jobs"])

    def test_framework_and_toolchain_tests_belong_to_compare(self):
        for path in ["tests/swift_toolchain.rs", "tests/framework_tests_swift.rs"]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["extra_compare"], ["swift"])
                self.assertEqual(plan["smoke_extra"], ["swift"])
                self.assertEqual(plan["smoke"], [])

    def test_ruby_emit_preserves_interpreted_family_and_adds_native_core(self):
        plan = ci.select(["src/emit/ruby.rs"])
        self.assertIn("compare-jruby", plan["jobs"])
        self.assertEqual(plan["smoke"], ["ruby", "jruby"])
        self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))

    def test_focused_tests_select_only_themselves(self):
        for binary in ci.SPINEL_TESTS:
            with self.subTest(binary=binary):
                plan = ci.select([f"tests/{binary}.rs"])
                self.assertEqual(plan["spinel_tests"], [binary])
                self.assertEqual(
                    self.extras(plan), set(ci.CORE) | {"spinel-framework"}
                )

    def test_param_binds_owns_lowering_drivers_and_database_runtime(self):
        for path in [
            "src/lower/arel/visitor.rs",
            "src/lower/model_to_library/adapter_emit.rs",
            "tests/param_binds.rs",
            "tests/param_binds_emit.rb",
            "tests/param_binds_raw_where.rb",
            "tests/param_binds_runtime.rb",
            "tests/param_binds_cruby_cache.rb",
            "tests/param_binds_spinel_cache.rb",
            "tests/param_binds_associations.rb",
            "tests/param_binds_nil.rb",
            "tests/support/emit_and_run.rs",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(
                    plan["spinel_tests"],
                    ci.PARAM_BIND_TESTS if path.startswith("src/") or path == "tests/support/emit_and_run.rs" else ["param_binds"],
                )
                self.assertEqual(
                    self.extras(plan), set(ci.CORE) | {"spinel-framework"}
                )
        # Generated-read ensure/finalize lives in the Ruby emitter.
        # Native core already runs for this path; the bind cleanup suite
        # must too when that file is the only change.
        self.assertEqual(
            ci.select(["src/emit/ruby/library.rs"])["spinel_tests"],
            ci.PARAM_BIND_TESTS,
        )
        self.assertEqual(
            ci.select(["runtime/spinel/db.rb"])["spinel_tests"],
            [
                "spinel_db_lease",
                *ci.PARAM_BIND_TESTS,
                "spinel_stmt_cache_lru",
                "db_sqlite_concurrency",
            ],
        )
        self.assertEqual(
            ci.select(["runtime/spinel/sqlite_adapter.rb"])["spinel_tests"],
            [
                "date_columns_spinel",
                "spinel_db_lease",
                *ci.PARAM_BIND_TESTS,
                "spinel_stmt_cache_lru",
                "db_sqlite_concurrency",
            ],
        )
        for path in [
            "README.md",
            "src/analyze/call.rs",
            "src/lower/rails.rs",
            "tests/spinel_web_push_crypto.rb",
        ]:
            with self.subTest(path=path):
                self.assertNotIn("param_binds", ci.select([path])["spinel_tests"])

    def test_jdbc_probes_select_the_existing_comparison_without_archives(self):
        for path, native_jobs, suites in [
            ("tests/support/jdbc_cleanup_failures.rb", set(), []),
            (
                "runtime/spinel/test/statement_cache_cases.rb",
                set(ci.CORE) | {"spinel-framework"},
                ["param_binds"],
            ),
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(self.extras(plan), native_jobs | {"compare-jruby"})
                self.assertIn("compare-jruby", plan["required"])
                self.assertEqual(plan["spinel_tests"], suites)
                self.assertEqual(plan["smoke"], [])
                self.assertEqual(plan["archives"], [])

    def test_param_bind_suite_drivers_select_their_native_harness(self):
        for suite in ci.PARAM_BIND_TESTS:
            for suffix in (".rs", ".rb", "_runtime.rb"):
                with self.subTest(suite=suite, suffix=suffix):
                    plan = ci.select(["tests/" + suite + suffix])
                    self.assertEqual(plan["spinel_tests"], [suite])
                    self.assertIn("spinel-framework", plan["jobs"])

    def test_runtime_owners_choose_asymmetric_focused_binaries(self):
        cases = {
            "runtime/spinel/web_push_crypto.rb": "spinel_web_push_crypto",
            "runtime/spinel/signed_cookies.rbs": "rails_compat_vectors_spinel",
            "runtime/spinel/active_record_equality_spinel.rb": "framework_tests_spinel",
            "runtime/spinel/param_builder.rb": "spinel_param_builder",
            "runtime/spinel/multipart.rb": "spinel_param_builder",
            "runtime/spinel/date.rb": "date_columns_spinel",
            "runtime/spinel/active_support_date_parsing.rb": "date_columns_spinel",
        }
        for path, binary in cases.items():
            with self.subTest(path=path):
                self.assertEqual(ci.select([path])["spinel_tests"], [binary])

    def test_interpreter_only_files_do_not_start_native_work(self):
        for path, targets in {
            "runtime/spinel/db_jruby.rb": ["jruby"],
            "runtime/spinel/markly_jruby.rb": ["jruby"],
            "runtime/spinel/scaffold/ruby_overlay/main.rb": ["ruby", "jruby"],
            "runtime/spinel/module_delegate.rb": ["ruby", "jruby"],
        }.items():
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["smoke"], targets)
                self.assertIn("compare-jruby", plan["jobs"])
                self.assertNotIn("spinel-build", plan["jobs"])
                self.assertEqual(plan["spinel_tests"], [])
        self.assertEqual(ci.select(["README.md"])["jobs"], ci.BASE)

    def test_shared_runtime_and_driver_inputs_select_real_harnesses(self):
        cases = {
            "runtime/ruby/action_controller/message_verifier.rbs": [
                "framework_tests_spinel",
                "rails_compat_vectors_spinel",
            ],
            "runtime/ruby/params.rb": [
                "framework_tests_spinel",
                "spinel_param_builder",
            ],
            "runtime/spinel/base64.rb": [
                "spinel_web_push_crypto",
                "rails_compat_vectors_spinel",
            ],
            "tests/spinel_db_lease.rb": ["spinel_db_lease"],
            "tests/spinel_stmt_cache_lru.rb": ["spinel_stmt_cache_lru"],
            "tests/support/db_concurrency_spinel.rb": ["db_sqlite_concurrency"],
            "runtime/spinel/db.rb": [
                "spinel_db_lease", *ci.PARAM_BIND_TESTS, "spinel_stmt_cache_lru",
                "db_sqlite_concurrency",
            ],
            "runtime/spinel/sqlite_adapter.rb": [
                "date_columns_spinel",
                "spinel_db_lease",
                *ci.PARAM_BIND_TESTS,
                "spinel_stmt_cache_lru",
                "db_sqlite_concurrency",
            ],
            "runtime/spinel/active_support_time_parsing.rb": [
                "spinel_db_lease", *ci.PARAM_BIND_TESTS, "spinel_stmt_cache_lru",
                "db_sqlite_concurrency",
            ],
            "runtime/spinel/date.rb": ["date_columns_spinel"],
            "runtime/spinel/date.rbs": ["date_columns_spinel"],
            "runtime/spinel/active_support_date_parsing.rb": ["date_columns_spinel"],
            "runtime/spinel/active_support_date_parsing.rbs": ["date_columns_spinel"],
            "runtime/spinel/active_record_date_serialization.rb": [
                "date_columns_spinel",
                "framework_tests_spinel",
            ],
            "runtime/spinel/active_record_date_serialization.rbs": [
                "date_columns_spinel"
            ],
            "tests/params_vectors/canon.rb": ["spinel_param_builder"],
            "tests/rails_compat_vectors.rb": ["rails_compat_vectors_spinel"],
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["spinel_tests"], expected)
                self.assertTrue(set(ci.CORE).issubset(plan["jobs"]))
                self.assertNotIn("campfire-smoke", plan["jobs"])

    def test_routing_union_is_order_independent(self):
        paths = [
            "runtime/spinel/web_push_crypto.rb",
            "runtime/spinel/fragment_cache.rb",
        ]
        self.assertEqual(
            ci.select(paths)["spinel_tests"], ci.select(paths[::-1])["spinel_tests"]
        )
        self.assertEqual(
            ci.select(paths)["spinel_tests"],
            ["framework_tests_spinel", "spinel_web_push_crypto"],
        )

    def test_union_and_deletion_paths_keep_each_owner(self):
        plan = ci.select(
            ["runtime/spinel/web_push_crypto.rb", "runtime/spinel/sqlite_adapter.rb"]
        )
        self.assertEqual(
            plan["spinel_tests"],
            [
                "date_columns_spinel",
                "spinel_web_push_crypto",
                "spinel_db_lease",
                *ci.PARAM_BIND_TESTS,
                "spinel_stmt_cache_lru",
                "db_sqlite_concurrency",
            ],
        )

    def test_wasm_changes_have_no_archive_or_spinel_fanout(self):
        plan = ci.select(["wasm/lib/driver.mjs"])
        self.assertTrue(plan["wasm"])
        self.assertIn("browser-smoke-ide", plan["required"])
        self.assertNotIn("build-site", plan["jobs"])

    def test_packaging_and_unknown_target_changes_keep_the_ruby_floor(self):
        for path in [
            "src/project.rs",
            "src/emit/newlang.rs",
            "tests/framework_tests_newlang.rs",
            "scripts/ci-plan.py",
            "Cargo.toml",
            "scripts/ci-reuse.py",
            "scripts/ci-archive-evidence.py",
            ".github/workflows/ci.yml",
        ]:
            with self.subTest(path=path):
                plan = ci.select([path])
                self.assertEqual(plan["jobs"], ci.BASE)
                self.assertEqual(plan["smoke"], [])
                self.assertEqual(plan["extra_compare"], [])
                self.assertFalse(plan["wasm"])

    def test_proven_project_owners_keep_their_consumers_without_other_targets(self):
        for scope in ["interpreted", "ruby-family"]:
            with self.subTest(scope=scope):
                plan = ci.select(["src/project.rs"], project_scope=scope)
                self.assertEqual(plan["smoke"], ["ruby", "jruby"])
                self.assertEqual(plan["extra_compare"], [])
                self.assertIn("compare-jruby", plan["required"])
                self.assertIn("writebook-inventory", plan["required"])
                self.assertIn("archive-results", plan["jobs"])
                self.assertNotIn("archive-results", plan["required"])
                self.assertFalse(plan["wasm"])
                self.assertFalse(plan["site"])
                if scope == "ruby-family":
                    self.assertTrue(set(ci.SPINEL11).issubset(plan["jobs"]))
                    self.assertEqual(plan["spinel_tests"], ci.SPINEL_TESTS)
                    self.assertEqual(plan["archives"], ["ruby", "jruby", "spinel"])
                else:
                    self.assertFalse(set(ci.SPINEL11).intersection(plan["jobs"]))
                    self.assertEqual(plan["archives"], ["ruby", "jruby"])
        for paths, options in [
            (["src/project.rs"], {"full": True}),
            (["src/project.rs", "src/emit/shared/ops.rs"], {}),
        ]:
            plan = ci.select(paths, project_scope="interpreted", **options)
            self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
            self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
            self.assertTrue(plan["extras_advisory"])
        self.assertEqual(
            ci.select(["src/project.rs"], project_scope="unknown")["jobs"],
            ci.BASE,
        )
        self.assertEqual(
            ci.select(["src/project.rs"], project_scope="unknown")["smoke"],
            [],
        )

    def test_cli_binary_does_not_expand_to_full(self):
        plan = ci.select(["src/bin/roundhouse.rs"])
        self.assertEqual(plan["jobs"], ci.BASE)
        self.assertEqual(plan["extra_compare"], [])
        interpreted = ci.select(
            ["src/project.rs", "src/bin/roundhouse.rs"],
            project_scope="interpreted",
        )
        self.assertEqual(interpreted["smoke"], ["ruby", "jruby"])

    def test_full_manual_and_publication_are_distinct(self):
        plan = ci.select([], full=True)
        self.assertEqual(plan["spinel_tests"], ci.SPINEL_TESTS)
        self.assertTrue(set(ci.SPINEL11).issubset(plan["jobs"]))
        self.assertIn("archive-results", plan["jobs"])
        self.assertNotIn("archive-results", plan["required"])
        self.assertIn("writebook-inventory", plan["required"])
        self.assertNotIn("deploy", plan["jobs"])
        published = ci.select([], full=True, publish=True)
        self.assertIn("assemble-site", published["required"])
        self.assertNotIn("archive-results", published["required"])
        with self.assertRaises(ValueError):
            ci.select([], publish=True)

    def test_shared_smoke_and_compare_harnesses_select_their_owners(self):
        plan = ci.select(["scripts/smoke"])
        self.assertEqual(plan["smoke"], ["rust", "typescript", "ruby", "jruby"])
        self.assertEqual(plan["smoke_extra"], list(ci.EXTRA_COMPARE_TARGETS))
        self.assertTrue(plan["extras_advisory"])
        self.assertTrue(plan["spinel"])
        self.assertEqual(plan["extra_compare"], [])
        self.assertEqual(
            ci.select(["tools/compare/src/main.rs"])["extra_compare"],
            ["crystal", "kotlin", "swift", "csharp", "go", "elixir", "python"],
        )
        self.assertTrue(
            set(ci.CORE).issubset(ci.select(["tools/compare/src/main.rs"])["jobs"])
        )

    def test_spinel_archive_and_campfire_paths_have_exact_heavy_owners(self):
        scaffold = ci.select(["runtime/spinel/scaffold/Makefile"])
        self.assertEqual(
            self.extras(scaffold),
            set(ci.CORE) | {"build-site", "spinel-smoke", "archive-results"},
        )
        self.assertEqual(scaffold["archives"], ["spinel"])
        compare = ci.select(["scripts/campfire-compare-diff.rb"])
        self.assertEqual(
            self.extras(compare),
            {
                "spinel-build",
                "campfire-spinel-build",
                "campfire-spinel-compare",
            },
        )
        db = ci.select(["scripts/campfire-db-differential"])
        self.assertEqual(
            self.extras(db), {"spinel-build", "campfire-spinel-db"}
        )
        archive = ci.select(["e2e/campfire/assets.spec.js"])
        self.assertEqual(
            self.extras(archive),
            {
                "spinel-build",
                "campfire-archive-build",
                "campfire-smoke",
                "campfire-smoke-docker",
                "archive-results",
            },
        )
        self.assertEqual(
            ci.select(["scripts/campfire-docker-files"])["jobs"],
            ci.select(["scripts/build-campfire-archive"])["jobs"],
        )
        self.assertNotIn(
            "campfire-spinel-compare",
            ci.select(["scripts/campfire-docker-files"])["jobs"],
        )


class Results(unittest.TestCase):
    def needs(self, plan):
        return {
            "plan": {"result": "success"},
            "compact-required": {"result": "success"},
            **{
                j: {
                    "result": "success",
                    "outputs": {
                        "execution": "success",
                        "default": "success",
                        "minor-gc": "success",
                        "verify-gen": "success",
                    },
                }
                for j in plan["jobs"]
            },
        }

    def test_selected_skips_and_missing_results_are_not_green(self):
        # Path-owned extras are ledger/advisory: incomplete, not a required fail.
        ledger = ci.select(["src/emit/go.rs"])
        self.assertTrue(ledger["extras_advisory"])
        for outcome in ["skipped", "cancelled", "failure", None]:
            with self.subTest(mode="ledger", outcome=outcome):
                needs = self.needs(ledger)
                needs["compare-extra"] = {"result": outcome}
                failures, complete = ci.check_results(ledger, needs)
                self.assertEqual(failures, [])
                self.assertFalse(complete)
        # Focus makes the same lane required for the fix round.
        focused = ci.select([], focus_extras=("go",))
        self.assertFalse(focused["extras_advisory"])
        for outcome in ["skipped", "cancelled", "failure", None]:
            with self.subTest(mode="focus", outcome=outcome):
                needs = self.needs(focused)
                needs["compare-extra"] = {"result": outcome}
                failures, complete = ci.check_results(focused, needs)
                self.assertTrue(failures)
                self.assertFalse(complete)

    def test_extra_failure_does_not_block_compact_publication_floor(self):
        plan = ci.select([], full=True, publish=True)
        needs = self.needs(plan)
        needs["compare-extra"]["result"] = "failure"
        self.assertFalse(ci.check_results(plan, needs, compact=True)[0])
        failures, complete = ci.check_results(plan, needs)
        self.assertEqual(failures, [])
        self.assertFalse(complete)
        needs["compare"]["result"] = "failure"
        self.assertTrue(ci.check_results(plan, needs, compact=True)[0])
        self.assertTrue(ci.check_results(plan, needs)[0])

    def test_speculative_success_cannot_hide_unit_or_compiler_failure(self):
        plan = ci.select([])
        for job in ["unit", "build-roundhouse"]:
            for result in ["failure", "cancelled", "skipped", None]:
                with self.subTest(job=job, result=result):
                    needs = self.needs(plan)
                    needs[job]["result"] = result
                    self.assertTrue(ci.check_results(plan, needs, compact=True)[0])
                    self.assertTrue(ci.check_results(plan, needs)[0])
                    self.assertFalse(ci.check_results(plan, needs)[1])

    def test_advisory_failure_is_visible_but_does_not_fail_required_gate(self):
        plan = ci.select([], full=True)
        needs = self.needs(plan)
        needs["spinel-build"]["outputs"]["execution"] = "failure"
        failures, complete = ci.check_results(plan, needs)
        self.assertEqual(failures, [])
        self.assertFalse(complete)
        needs["spinel-smoke"]["result"] = "skipped"
        self.assertFalse(ci.check_results(plan, needs)[1])

    def test_archive_results_report_never_fails_summary_gate(self):
        # Repro: PR ci:full run 37652830796 — every producer succeeded, but
        # GitHub left archive-results `abandoned` and CI summary exited 1.
        plan = ci.select([], full=True)
        self.assertIn("archive-results", plan["jobs"])
        self.assertNotIn("archive-results", plan["required"])
        self.assertEqual(ci.REPORTING, {"archive-results"})
        for outcome in ["abandoned", "skipped", "failure", "cancelled", None]:
            with self.subTest(outcome=outcome):
                needs = self.needs(plan)
                needs["archive-results"] = {"result": outcome}
                failures, complete = ci.check_results(plan, needs)
                self.assertEqual(failures, [])
                self.assertFalse(complete)
        # Publication still selects assemble-site as required; the report job
        # stays evidence-only at the summary gate.
        published = ci.select([], full=True, publish=True)
        needs = self.needs(published)
        needs["archive-results"] = {"result": "abandoned"}
        self.assertEqual(ci.check_results(published, needs)[0], [])
        self.assertFalse(ci.check_results(published, needs)[1])

    def test_assembly_failure_cannot_issue_checkpoint(self):
        plan = ci.select([], full=True, publish=True)
        needs = self.needs(plan)
        needs["assemble-site"]["result"] = "failure"
        self.assertTrue(ci.check_results(plan, needs)[0])
        self.assertFalse(ci.check_results(plan, needs)[1])

    def test_each_gc_mode_must_actually_pass_for_completion(self):
        # Full selects advisory ledger extras; those never count as complete
        # under CoE (matrix cells). Probe GC-mode receipts on the Spinel lane.
        plan = ci.select([], spinel_lane=True)
        self.assertEqual(ci.check_results(plan, self.needs(plan)), ([], True))
        for mode in ["default", "minor-gc", "verify-gen"]:
            for status in ["failure", "cancelled", "", None]:
                with self.subTest(mode=mode, status=status):
                    needs = self.needs(plan)
                    outputs = needs["campfire-spinel-compare"]["outputs"]
                    if status is None:
                        del outputs[mode]
                    else:
                        outputs[mode] = status
                    self.assertEqual(ci.check_results(plan, needs), ([], False))

    def test_advisory_ledger_extras_never_claim_complete_under_coe(self):
        # continue-on-error makes needs.*.result=success even when a matrix
        # cell failed; without a trustworthy job-level proof, complete stays
        # false while extras remain advisory.
        ledger = ci.select(["src/emit/go.rs"])
        self.assertTrue(ledger["extras_advisory"])
        needs = self.needs(ledger)
        needs["compare-extra"] = {"result": "success", "outputs": {"execution": "success"}}
        needs["smoke-extra"] = {"result": "success", "outputs": {"execution": "success"}}
        failures, complete = ci.check_results(ledger, needs)
        self.assertEqual(failures, [])
        self.assertFalse(complete)
        full = ci.select([], full=True)
        self.assertTrue(full["extras_advisory"])
        self.assertEqual(ci.check_results(full, self.needs(full))[0], [])
        self.assertFalse(ci.check_results(full, self.needs(full))[1])
        focused = ci.select([], focus_extras=("go",))
        self.assertFalse(focused["extras_advisory"])
        self.assertEqual(ci.check_results(focused, self.needs(focused)), ([], True))

    def test_unselected_jobs_may_skip_but_planner_must_succeed(self):
        for lane in ({}, {"spinel_lane": True}, {"full": True}):
            with self.subTest(lane=lane):
                plan = ci.select([], **lane)
                needs = self.needs(plan)
                for job in ["build-wasm", "compare", "browser-smoke-typescript", "assemble-site"]:
                    if job not in plan["jobs"]:
                        needs[job] = {"result": "skipped"}
                failures, complete = ci.check_results(plan, needs)
                self.assertEqual(failures, [])
                # Full keeps advisory ledger extras → incomplete; floor /
                # Spinel-lane plans can still claim complete.
                if plan.get("extras_advisory"):
                    self.assertFalse(complete)
                else:
                    self.assertTrue(complete)
                self.assertEqual(ci.check_results(plan, needs, compact=True), ([], True))
                needs["plan"]["result"] = "failure"
                self.assertTrue(ci.check_results(plan, needs)[0])

    def test_compact_gate_ignores_unselected_publication_lanes(self):
        plan = ci.select(["src/analyze/call.rs"])
        needs = self.needs(plan)
        needs["compare"] = {"result": "skipped"}
        needs["browser-smoke-typescript"] = {"result": "skipped"}
        self.assertEqual(ci.check_results(plan, needs, compact=True), ([], True))
        owned = ci.select(["src/emit/rust.rs"])
        needs = self.needs(owned)
        needs["compare"]["result"] = "failure"
        self.assertTrue(ci.check_results(owned, needs, compact=True)[0])


class Workflow(unittest.TestCase):
    def test_jruby_runs_cleanup_after_runtime_setup(self):
        workflow = (Path(__file__).parents[1] / ".github/workflows/ci.yml").read_text()
        job = re.search(
            r"(?ms)^  compare-jruby:\n(.*?)(?=^  [\w-]+:|\Z)", workflow
        )[1]
        probe = "jruby tests/support/jdbc_cleanup_failures.rb"
        steps = re.split(r"(?m)^      - ", job)[1:]
        step = next((s for s in steps if f"          {probe}\n" in s), None)
        self.assertIsNotNone(step, "compare-jruby must execute the cleanup probe")
        self.assertNotIn("continue-on-error:", job)
        self.assertNotIn("\n        if:", step)
        install = "jruby -S gem install jdbc-sqlite3 --no-document"
        self.assertIn(f"\n          {install}\n", step)
        self.assertLess(
            job.index("java-version: '21'"), job.index("ruby-version: 'jruby-10.0'")
        )
        self.assertLess(job.index("ruby-version: 'jruby-10.0'"), job.index(install))
        self.assertLess(step.index(install), step.index(probe))
        self.assertLess(job.index(probe), job.index("ruby-version: ${{ env.MRI_RUBY }}"))


class MergeTree(unittest.TestCase):
    def test_fixture_ignores_inherited_git_locations(self):
        with git_repository() as (outer, outer_git):
            (outer / "sentinel").write_text("unchanged\n")
            outer_git("add", ".")
            outer_git("commit", "-m", "outer")
            outer_head = outer_git("rev-parse", "HEAD")
            locations = {
                "GIT_DIR": str(outer / ".git"),
                "GIT_COMMON_DIR": str(outer / ".git"),
                "GIT_WORK_TREE": str(outer),
                "GIT_INDEX_FILE": str(outer / ".git/index"),
                "GIT_OBJECT_DIRECTORY": str(outer / ".git/objects"),
                "GIT_ALTERNATE_OBJECT_DIRECTORIES": str(outer / ".git/objects"),
                "GIT_PREFIX": "wrong/",
            }
            with patch.dict(os.environ, {**locations, "CI_FIXTURE_MARKER": "kept"}):
                with git_repository() as (root, git):
                    self.assertEqual(Path(git("rev-parse", "--show-toplevel")), root)
                    self.assertEqual(os.environ["CI_FIXTURE_MARKER"], "kept")
                    (root / "README.md").write_text("base\n")
                    git("add", ".")
                    git("commit", "-m", "base")
                    base = git("rev-parse", "HEAD")
                    (root / "README.md").write_text("changed\n")
                    git("add", ".")
                    git("commit", "-m", "docs")
                    head = git("rev-parse", "HEAD")
                    self.assertEqual(
                        ci.changed_inputs({"before": base}, "push", head),
                        (["README.md"], None),
                    )
                    event_path = root / "event.json"
                    event_path.write_text(json.dumps({"before": base}))
                    output = subprocess.check_output(
                        ["python3", "-B", ci.__file__, "plan"],
                        env={
                            **os.environ,
                            "GITHUB_EVENT_PATH": str(event_path),
                            "GITHUB_EVENT_NAME": "push",
                            # Not inherited: canonical main selects the Spinel lane, not this feature ref.
                            "GITHUB_REF": "refs/heads/feature",
                            "GITHUB_SHA": head,
                            "GITHUB_OUTPUT": os.devnull,
                            "GITHUB_STEP_SUMMARY": os.devnull,
                            "CI_FULL": "false",
                            "CI_PUBLISH": "false",
                            "CI_SPINEL_REVISION": "a" * 40,
                        },
                        text=True,
                    )
                    jobs = next(
                        line[5:]
                        for line in output.splitlines()
                        if line.startswith("jobs=")
                    )
                    self.assertEqual(json.loads(jobs), ci.BASE)
                self.assertEqual(
                    {name: os.environ[name] for name in locations}, locations
                )
            self.assertEqual(outer_git("rev-parse", "HEAD"), outer_head)
            self.assertEqual(outer_git("status", "--porcelain"), "")

    def test_diff_tracks_both_rename_owners_and_deletions(self):
        with git_repository() as (root, git):
            (root / "src/emit").mkdir(parents=True)
            (root / "src/emit/go.rs").write_text("old owner\n")
            git("add", ".")
            git("commit", "-m", "base")
            base = git("rev-parse", "HEAD")
            git("switch", "-c", "feature")
            (root / "src/emit/go.rs").rename(root / "src/emit/swift.rs")
            git("add", ".")
            git("commit", "-m", "rename")
            head = git("rev-parse", "HEAD")
            git("switch", "-")
            git("merge", "--no-ff", "feature", "-m", "merge")
            sha = git("rev-parse", "HEAD")
            event = {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}}
            paths, scope = ci.changed_inputs(event, "pull_request", sha)
            self.assertIsNone(scope)
            self.assertEqual(set(paths), {"src/emit/go.rs", "src/emit/swift.rs"})
            self.assertEqual(ci.select(paths)["extra_compare"], ["swift", "go"])
            with self.assertRaises(ValueError):
                ci.changed_inputs(event, "pull_request", head)

    def test_pr_head_checkout_diffs_against_the_event_base(self):
        with git_repository() as (root, git):
            (root / "src/emit").mkdir(parents=True)
            (root / "src/emit/go.rs").write_text("old owner\n")
            git("add", ".")
            git("commit", "-m", "base")
            base = git("rev-parse", "HEAD")
            git("switch", "-c", "feature")
            (root / "src/emit/go.rs").write_text("new owner\n")
            git("add", ".")
            git("commit", "-m", "feature")
            head = git("rev-parse", "HEAD")
            git("checkout", "--detach", head)
            event = {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}}
            paths, scope = ci.changed_inputs(event, "pull_request", head)
            self.assertIsNone(scope)
            self.assertEqual(paths, ["src/emit/go.rs"])

    def test_merge_tree_survives_a_newer_main_than_the_event_base(self):
        with git_repository() as (root, git):
            (root / "src/analyze").mkdir(parents=True)
            (root / "src/analyze/call.rs").write_text("old\n")
            git("add", ".")
            git("commit", "-m", "base")
            event_base = git("rev-parse", "HEAD")
            git("switch", "-c", "feature")
            (root / "src/analyze/call.rs").write_text("feature\n")
            git("add", ".")
            git("commit", "-m", "feature")
            head = git("rev-parse", "HEAD")
            git("switch", "-")
            (root / "README.md").write_text("main moved\n")
            git("add", ".")
            git("commit", "-m", "main advanced")
            git("merge", "--no-ff", "feature", "-m", "merge")
            sha = git("rev-parse", "HEAD")
            event = {
                "pull_request": {"base": {"sha": event_base}, "head": {"sha": head}}
            }
            paths, scope = ci.changed_inputs(event, "pull_request", sha)
            self.assertIsNone(scope)
            self.assertEqual(paths, ["src/analyze/call.rs"])
            self.assertEqual(ci.select(paths)["jobs"], ci.BASE)
            stale = {"pull_request": {"base": {"sha": head}, "head": {"sha": head}}}
            paths, scope = ci.changed_inputs(stale, "pull_request", sha)
            self.assertEqual(paths, ["src/analyze/call.rs"])


class ProjectScope(unittest.TestCase):
    source = (
        "fn target_files() {\n    shared();\n}\n\n"
        "fn ruby_family_runtime_files() {\n    interpreted();\n}\n\n"
        "fn spinel_files() {\n    native();\n}\n"
    )

    def test_body_ownership_and_union(self):
        interpreted = self.source.replace("interpreted();", "new_interpreted();")
        native = self.source.replace("native();", "new_native();")
        self.assertEqual(
            ci.project_change_scope(self.source, interpreted), "interpreted"
        )
        self.assertEqual(ci.project_change_scope(self.source, native), "ruby-family")
        self.assertEqual(
            ci.project_change_scope(
                self.source, native.replace("interpreted();", "new_interpreted();")
            ),
            "ruby-family",
        )
        self.assertEqual(ci.project_change_scope(native, self.source), "ruby-family")

    def test_all_builder_owners_and_raw_literal_fallbacks(self):
        for name, scope in {
            "ruby_runtime_files": "interpreted",
            "jruby_runtime_files": "interpreted",
            "ruby_family_runtime_files": "interpreted",
            "spinel_files": "ruby-family",
            "spin_shape": "ruby-family",
        }.items():
            before = f"fn {name}() {{\n    old();\n}}\n"
            after = before.replace("old();", "new();")
            with self.subTest(name=name):
                self.assertEqual(ci.project_change_scope(before, after), scope)
            for prefix in ["r", "br", "cr"]:
                for hashes in ["", "#", "###"]:
                    raw = f'let text = {prefix}{hashes}"old"{hashes};'
                    literal = before.replace("old();", raw)
                    for first, second in [
                        (before, literal),
                        (literal, before),
                        (literal, literal.replace('"old"', '"new"')),
                    ]:
                        with self.subTest(name=name, raw=raw, first=first):
                            self.assertIsNone(ci.project_change_scope(first, second))

    def test_unknown_shared_signatures_and_ambiguous_shapes_do_not_narrow(self):
        for changed in [
            self.source.replace("shared();", "new_shared();"),
            self.source.replace("spinel_files()", "spinel_files(app: &App)"),
            self.source.replace("fn spinel_files", "pub fn spinel_files"),
            self.source.replace("    native();", "native();"),
            self.source.replace("    native();", '    let text = r#"native();"#;'),
            self.source.replace("    native();", "    /* native(); */"),
            self.source.replace("    native();", "    /*\n}\nfn shared() {\n    */"),
            self.source + "\nfn shared_helper() {\n    work();\n}\n",
            self.source + "\nfn spinel_files() {\n    duplicate();\n}\n",
            self.source.replace("fn spinel_files() {\n    native();\n}\n", ""),
            self.source,
        ]:
            with self.subTest(changed=changed):
                self.assertIsNone(ci.project_change_scope(self.source, changed))
        self.assertIsNone(
            ci.project_change_scope(
                self.source,
                self.source.replace("native();", "new_native();").replace(
                    "shared();", "new_shared();"
                ),
            )
        )
        self.assertEqual(
            ci.select(["src/project.rs"], project_scope=None)["jobs"],
            ci.BASE,
        )

    def test_builder_text_inside_strings_or_comments_is_not_a_rust_item(self):
        fake = "fn ruby_runtime_files() {\n    cross_target();\n}\n"
        for opening, closing in [
            ('const SHARED: &str = r##"\n', '"##;\n'),
            ('const SHARED: &str = "\n', '";\n'),
            ('const SHARED: &str = br#"\n', '"#;\n'),
            ("/*\n", "*/\n"),
            ("shared_macro! {\n", "}\n"),
            ("shared_macro!(\n", ");\n"),
            ("shared_macro![\n", "];\n"),
        ]:
            source = opening + fake + closing + self.source
            with self.subTest(opening=opening):
                self.assertIsNone(
                    ci.project_change_scope(
                        source, source.replace("cross_target();", "different();")
                    )
                )

    def test_real_http_auth_registration_is_family_owned(self):
        source = (Path(__file__).parents[1] / "src/project.rs").read_text()
        registration = """    // HTTP Token/Basic auth sidecar — the ActionController::Base reopen in
    // runtime/http_authentication.rb (ruby family only). It types the
    // block parameters the helpers yield, which the app's blocks compare.
    {
        let rbs = crate::runtime_files::read_to_string("runtime/spinel/http_authentication.rbs")
            .map_err(|e| format!("read runtime/spinel/http_authentication.rbs: {e}"))?;
        files.push(("sig/runtime/http_authentication.rbs".to_string(), rbs));
    }

"""
        self.assertEqual(source.count(registration), 1)
        self.assertEqual(
            ci.project_change_scope(source.replace(registration, ""), source),
            "ruby-family",
        )

    def test_git_uses_whole_event_trees_and_rejects_mode_changes(self):
        with git_repository() as (root, git):
            (root / "src").mkdir()
            project = root / "src/project.rs"
            project.write_text(self.source)
            git("add", ".")
            git("commit", "-m", "base")
            base = git("rev-parse", "HEAD")
            git("switch", "-c", "feature")
            project.write_text(self.source.replace("native();", "new_native();"))
            git("add", ".")
            git("commit", "-m", "native change")
            native = git("rev-parse", "HEAD")
            project.write_text(
                project.read_text().replace("shared();", "new_shared();")
            )
            git("add", ".")
            git("commit", "-m", "shared change")
            head = git("rev-parse", "HEAD")
            git("switch", "-")
            git("merge", "--no-ff", "feature", "-m", "merge")
            sha = git("rev-parse", "HEAD")
            event = {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}}
            with self.assertRaises(ValueError):
                ci.changed_inputs({"before": base}, "push", native)
            git("checkout", "--detach", native)
            paths, scope = ci.changed_inputs({"before": base}, "push", native)
            self.assertEqual(paths, ["src/project.rs"])
            self.assertEqual(scope, "ruby-family")
            event_path = root / "event.json"
            event_path.write_text(json.dumps({"before": base}))
            output = subprocess.check_output(
                ["python3", "-B", ci.__file__, "plan"],
                env={
                    **os.environ,
                    "GITHUB_EVENT_PATH": str(event_path),
                    "GITHUB_EVENT_NAME": "push",
                    # Not inherited: canonical main selects the Spinel lane, not this feature ref.
                    "GITHUB_REF": "refs/heads/feature",
                    "GITHUB_SHA": native,
                    "GITHUB_OUTPUT": os.devnull,
                    "CI_FULL": "false",
                    "CI_PUBLISH": "false",
                    "CI_SPINEL_REVISION": "a" * 40,
                },
                text=True,
            )
            plan = json.loads(
                next(
                    line[5:] for line in output.splitlines() if line.startswith("plan=")
                )
            )
            self.assertEqual(plan["smoke"], ["ruby", "jruby"])
            self.assertFalse(plan["wasm"])
            git("checkout", "--detach", sha)
            self.assertIsNone(ci.changed_inputs(event, "pull_request", sha)[1])
            git("checkout", "--detach", native)
            project.chmod(0o755)
            git("add", ".")
            git("commit", "-m", "mode change")
            self.assertIsNone(
                ci.changed_inputs({"before": base}, "push", git("rev-parse", "HEAD"))[1]
            )


if __name__ == "__main__":
    unittest.main()
