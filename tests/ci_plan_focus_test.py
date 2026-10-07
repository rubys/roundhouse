"""Narrow focus-label and ledger-advisory contracts for scripts/ci-plan.py."""

import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "ci_plan", Path(__file__).parents[1] / "scripts/ci-plan.py"
)
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


def plan_pull_request(labels, *, paths=None, changed_inputs=None):
    """Run `ci-plan.py plan` for a PR event with the given label names."""
    if changed_inputs is None:
        changed_inputs = (paths if paths is not None else ["README.md"], None)
    with tempfile.TemporaryDirectory() as directory:
        event = Path(directory) / "event.json"
        event.write_text(
            json.dumps(
                {
                    "pull_request": {
                        "labels": [{"name": label} for label in labels],
                    }
                }
            )
        )
        env = {
            "GITHUB_EVENT_PATH": str(event),
            "GITHUB_EVENT_NAME": "pull_request",
            "GITHUB_SHA": "1" * 40,
            "CI_SPINEL_REVISION": "2" * 40,
        }
        patch_inputs = (
            patch.object(ci, "changed_inputs", side_effect=changed_inputs)
            if isinstance(changed_inputs, BaseException)
            else patch.object(ci, "changed_inputs", return_value=changed_inputs)
        )
        with (
            patch.dict(os.environ, env, clear=True),
            patch("sys.argv", ["ci-plan.py", "plan"]),
            patch_inputs,
            patch.object(ci, "write_outputs") as output,
        ):
            assert ci.main() == 0
        return output.call_args.args[0]["plan"]


class FocusLabels(unittest.TestCase):
    """Narrow focus labels: BASE + selected lanes; path ownership suppressed."""

    EXTRA_FOCUS_JOBS = [
        *ci.BASE,
        "compare-extra",
        "build-site",
        "smoke-extra",
        "archive-results",
    ]

    def assert_extra_focus(self, plan, langs):
        self.assertEqual(plan["jobs"], self.EXTRA_FOCUS_JOBS)
        self.assertEqual(plan["extra_compare"], list(langs))
        self.assertEqual(plan["smoke_extra"], list(langs))
        self.assertEqual(plan["smoke"], [])
        self.assertEqual(plan["compare"], [])
        self.assertEqual(plan["archives"], list(langs))
        self.assertFalse(plan["wasm"])
        self.assertFalse(plan["site"])
        self.assertFalse(plan["spinel"])
        self.assertFalse(plan["extras_advisory"])
        self.assertIn("compare-extra", plan["required"])
        self.assertIn("smoke-extra", plan["required"])
        self.assertNotIn("archive-results", plan["required"])
        self.assertNotIn("compare-jruby", plan["jobs"])
        self.assertNotIn("build-wasm", plan["jobs"])
        self.assertFalse(set(ci.SPINEL11).intersection(plan["jobs"]))

    def test_extra_compare_targets_and_labels(self):
        self.assertEqual(
            ci.EXTRA_COMPARE_TARGETS,
            ["crystal", "kotlin", "swift", "csharp", "go", "elixir", "python"],
        )
        self.assertEqual(ci.CI_JRUBY, "ci:jruby")
        self.assertEqual(ci.CI_SPINEL, "ci:spinel")
        self.assertEqual(ci.LEDGER_EXTRAS, {"compare-extra", "smoke-extra"})

    def test_parse_coverage_labels(self):
        parsed = ci.parse_coverage_labels(["ci:swift", "ci:go", "ci:jruby", "ci:spinel"])
        self.assertFalse(parsed.full)
        self.assertTrue(parsed.focus_jruby)
        self.assertTrue(parsed.focus_spinel)
        self.assertEqual(parsed.focus_extras, ("swift", "go"))
        extras = ci.parse_coverage_labels(["ci:extras"])
        self.assertEqual(extras.focus_extras, tuple(ci.EXTRA_COMPARE_TARGETS))

    def test_single_lang_and_extras_focus_are_required(self):
        for lang in ci.EXTRA_COMPARE_TARGETS:
            with self.subTest(lang=lang):
                self.assert_extra_focus(ci.select([], focus_extras=(lang,)), (lang,))
        self.assert_extra_focus(
            ci.select([], focus_extras=tuple(ci.EXTRA_COMPARE_TARGETS)),
            ci.EXTRA_COMPARE_TARGETS,
        )

    def test_focus_suppresses_path_ownership(self):
        plan = ci.select(
            ["src/emit/go.rs", "wasm/lib/driver.mjs"],
            focus_extras=("swift",),
        )
        self.assert_extra_focus(plan, ("swift",))

    def test_full_overrides_focus(self):
        plan = ci.select(["README.md"], full=True, focus_extras=("swift",))
        self.assertEqual(plan["compare"], ["rust", "typescript"])
        self.assertTrue(plan["extras_advisory"])
        self.assertNotIn("compare-extra", plan["required"])
        self.assertNotIn("smoke-extra", plan["required"])

    def test_publish_requires_full_even_with_focus(self):
        with self.assertRaisesRegex(ValueError, "publication requires full"):
            ci.select([], focus_extras=("go",), publish=True)
        with self.assertRaisesRegex(ValueError, "publication requires full"):
            ci.select([], focus_spinel=True, publish=True)
        with self.assertRaisesRegex(ValueError, "publication requires full"):
            ci.select([], spinel_lane=True, publish=True)

    def test_path_extras_are_advisory_ledger(self):
        plan = ci.select(["src/emit/go.rs"])
        self.assertEqual(plan["extra_compare"], ["go"])
        self.assertEqual(plan["smoke_extra"], ["go"])
        self.assertEqual(plan["smoke"], [])
        self.assertTrue(plan["extras_advisory"])
        self.assertNotIn("compare-extra", plan["required"])
        self.assertNotIn("smoke-extra", plan["required"])
        self.assertTrue(set(ci.BASE).issubset(plan["required"]))

    def test_ci_jruby_focus_lane(self):
        plan = ci.select([], focus_jruby=True)
        self.assertIn("compare-jruby", plan["required"])
        self.assertIn("smoke", plan["required"])
        self.assertEqual(plan["smoke"], ["jruby"])
        self.assertEqual(plan["smoke_extra"], [])
        self.assertFalse(plan["extras_advisory"])
        self.assertNotIn("compare-extra", plan["jobs"])
        self.assertFalse(plan["wasm"])

    def test_ci_spinel_focus_is_core_and_required(self):
        plan = ci.select([], focus_spinel=True)
        self.assertEqual(
            [j for j in plan["jobs"] if j not in ci.BASE],
            [
                "build-spinel",
                "toolchain-spinel",
                "compare-spinel",
                "framework-tests-spinel",
                "build-site",
                "archive-results",
            ],
        )
        for job in ci.CORE + ["framework-tests-spinel"]:
            self.assertIn(job, plan["required"])
            self.assertNotIn(job, plan["advisory"])
        self.assertFalse(plan["spinel_advisory"])
        self.assertNotIn("campfire-compare-spinel", plan["jobs"])
        self.assertEqual(plan["spinel_tests"], ci.SPINEL_TESTS)

    def test_main_spinel_lane_stays_advisory_full_suite(self):
        plan = ci.select([], spinel_lane=True)
        self.assertEqual(plan["jobs"], ci.SPINEL_LANE)
        self.assertNotIn("compare-spinel", plan["required"])
        self.assertTrue(plan["spinel_advisory"])
        self.assertTrue(set(ci.CORE).issubset(plan["advisory"]))
        self.assertTrue(set(ci.SPINEL11).issubset(plan["jobs"]))

    def test_rust_and_typescript_compare_independently(self):
        rust = ci.select(["src/emit/rust.rs"])
        self.assertEqual(rust["compare"], ["rust"])
        self.assertEqual(rust["smoke"], ["rust"])
        self.assertNotIn("browser-smoke-typescript", rust["jobs"])
        ts = ci.select(["src/emit/typescript.rs"])
        self.assertEqual(ts["compare"], ["typescript"])
        self.assertEqual(ts["smoke"], ["typescript"])
        self.assertIn("browser-smoke-typescript", ts["jobs"])

    def test_pr_events_drive_focus_and_unknown_keeps_focus(self):
        plan = plan_pull_request(
            ["ci:kotlin", "ci:python"],
            changed_inputs=(["src/emit/go.rs"], None),
        )
        self.assert_extra_focus(plan, ("kotlin", "python"))
        self.assert_extra_focus(plan_pull_request(["ci:extras"]), ci.EXTRA_COMPARE_TARGETS)
        full = plan_pull_request(["ci:full", "ci:swift"])
        self.assertTrue(full["extras_advisory"])
        unknown = plan_pull_request(["ci:go"], changed_inputs=ValueError("no tree"))
        self.assert_extra_focus(unknown, ("go",))
        jruby = plan_pull_request(["ci:jruby"])
        self.assertEqual(jruby["smoke"], ["jruby"])
        self.assertIn("compare-jruby", jruby["required"])


if __name__ == "__main__":
    unittest.main()
