"""Narrow focus-label contracts for scripts/ci-plan.py."""

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
    """Run `ci-plan.py plan` for a PR event with the given label names.

    `changed_inputs` is either a `(paths, project_scope)` return value or an
    exception instance raised from `changed_inputs()`.
    """
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
    """Narrow focus labels: BASE + selected extras; path ownership suppressed."""

    FOCUS_JOBS = [
        *ci.BASE,
        "compare-extra",
        "build-site",
        "smoke",
        "archive-results",
    ]

    def assert_focus_plan(self, plan, langs):
        self.assertEqual(plan["jobs"], self.FOCUS_JOBS)
        self.assertEqual(plan["extra_compare"], list(langs))
        self.assertEqual(plan["smoke"], list(langs))
        self.assertEqual(plan["archives"], list(langs))
        self.assertFalse(plan["wasm"])
        self.assertFalse(plan["site"])
        self.assertFalse(plan["spinel"])
        self.assertEqual(plan["spinel_tests"], [])
        self.assertIn("compare-extra", plan["required"])
        self.assertIn("smoke", plan["required"])
        self.assertIn("build-site", plan["required"])
        self.assertIn("archive-results", plan["jobs"])
        self.assertNotIn("archive-results", plan["required"])
        self.assertNotIn("compare", plan["jobs"])
        self.assertNotIn("compare-jruby", plan["jobs"])
        self.assertNotIn("build-wasm", plan["jobs"])
        self.assertNotIn("writebook-inventory", plan["jobs"])
        self.assertFalse(set(ci.SPINEL11).intersection(plan["jobs"]))
        self.assertTrue(any("ci focus:" in reason for reason in plan["reasons"]))

    def test_extra_compare_targets_match_the_seven_sdk_langs(self):
        self.assertEqual(
            ci.EXTRA_COMPARE_TARGETS,
            ["crystal", "kotlin", "swift", "csharp", "go", "elixir", "python"],
        )
        self.assertEqual(
            set(ci.CI_FOCUS_BY_LABEL),
            {f"ci:{t}" for t in ci.EXTRA_COMPARE_TARGETS},
        )
        self.assertEqual(ci.CI_EXTRAS, "ci:extras")
        # Deferred: ci:jruby is not a focus label (extension-point comment only).
        self.assertNotIn("ci:jruby", ci.CI_FOCUS_BY_LABEL)

    def test_parse_coverage_labels_unions_langs_and_respects_full(self):
        parsed = ci.parse_coverage_labels(["ci:swift", "ci:go", "ci:draft"])
        self.assertFalse(parsed.full)
        self.assertFalse(parsed.spinel_lane)
        self.assertEqual(parsed.focus_extras, ("swift", "go"))
        extras = ci.parse_coverage_labels(["ci:extras", "ci:swift"])
        self.assertEqual(extras.focus_extras, tuple(ci.EXTRA_COMPARE_TARGETS))
        full = ci.parse_coverage_labels(["ci:full", "ci:swift"], env_full=False)
        self.assertTrue(full.full)
        self.assertEqual(full.focus_extras, ("swift",))
        env_full = ci.parse_coverage_labels(["ci:swift"], env_full=True)
        self.assertTrue(env_full.full)
        deferred = ci.parse_coverage_labels(["ci:jruby", "ci:spinel"])
        self.assertEqual(deferred.focus_extras, ())
        self.assertTrue(deferred.spinel_lane)

    def test_single_lang_focus_selects_compare_extra_and_smoke(self):
        for lang in ci.EXTRA_COMPARE_TARGETS:
            with self.subTest(lang=lang):
                self.assert_focus_plan(ci.select([], focus_extras=(lang,)), (lang,))

    def test_ci_extras_selects_all_seven(self):
        self.assert_focus_plan(
            ci.select([], focus_extras=tuple(ci.EXTRA_COMPARE_TARGETS)),
            ci.EXTRA_COMPARE_TARGETS,
        )

    def test_focus_suppresses_path_ownership_and_spinel(self):
        plan = ci.select(
            ["src/emit/go.rs", "wasm/lib/driver.mjs", "runtime/ruby/active_record.rb"],
            focus_extras=("swift",),
            spinel_lane=True,
        )
        self.assert_focus_plan(plan, ("swift",))
        self.assertNotEqual(plan["extra_compare"], ["go"])

    def test_full_overrides_focus(self):
        plan = ci.select(["README.md"], full=True, focus_extras=("swift",))
        self.assertEqual(plan["smoke"], ci.TARGETS)
        self.assertTrue(plan["wasm"])
        self.assertTrue(set(ci.SPINEL11).issubset(plan["jobs"]))
        self.assertEqual(plan["extra_compare"], list(ci.EXTRA_COMPARE_TARGETS))

    def test_pr_event_focus_labels_drive_plan(self):
        plan = plan_pull_request(
            ["ci:kotlin", "ci:python"],
            changed_inputs=(["src/emit/go.rs"], None),
        )
        self.assert_focus_plan(plan, ("kotlin", "python"))

    def test_pr_event_ci_extras_and_full_precedence(self):
        self.assert_focus_plan(
            plan_pull_request(["ci:extras"]),
            ci.EXTRA_COMPARE_TARGETS,
        )
        plan = plan_pull_request(["ci:full", "ci:swift"])
        self.assertEqual(plan["smoke"], ci.TARGETS)
        self.assertTrue(plan["wasm"])

    def test_unknown_inputs_keep_focus_instead_of_spinel_fallback(self):
        plan = plan_pull_request(
            ["ci:go"],
            changed_inputs=ValueError("no tree"),
        )
        self.assert_focus_plan(plan, ("go",))
        self.assertTrue(
            any("focus labels keep BASE+selected extras" in r for r in plan["reasons"])
        )


if __name__ == "__main__":
    unittest.main()
