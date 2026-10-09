"""The Rails-resolved Campfire route snapshot is an independent coverage denominator."""

from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CI = ROOT / ".github/workflows/ci.yml"
INVENTORY = ROOT / "e2e/campfire/route-inventory.json"


class CampfireRouteInventoryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.inventory = json.loads(INVENTORY.read_text())
        workflow = CI.read_text()
        match = re.search(r"^  CAMPFIRE_SHA: ([0-9a-f]{40})$", workflow, re.MULTILINE)
        if match is None:
            raise AssertionError("CI must pin the Campfire source SHA")
        cls.pinned_sha = match.group(1)

    def test_inventory_is_from_the_exact_ci_campfire_revision(self):
        self.assertEqual(self.inventory["schema_version"], 1)
        self.assertEqual(self.inventory["source"]["campfire_sha"], self.pinned_sha)
        self.assertRegex(self.inventory["source"]["routes_sha256"], r"^[0-9a-f]{64}$")
        self.assertRegex(self.inventory["source"]["gemfile_lock_sha256"], r"^[0-9a-f]{64}$")
        self.assertTrue(self.inventory["source"]["rails_revision"])

    def test_inventory_preserves_all_resolved_routes_and_scopes(self):
        routes = self.inventory["routes"]
        self.assertEqual(len(routes), 178)
        derived = {}
        for route in routes:
            derived[route["scope"]] = derived.get(route["scope"], 0) + 1
        self.assertEqual(self.inventory["counts"], derived)
        self.assertEqual(
            derived,
            {
                "action_cable": 1,
                "action_mailbox": 6,
                "active_storage": 9,
                "campfire": 150,
                "rails_health": 1,
                "rails_conductor": 8,
                "turbo_native": 3,
            },
        )
        self.assertEqual([route["order"] for route in routes], list(range(1, 179)))
        self.assertTrue(all(route["path"].startswith("/") for route in routes))
        self.assertTrue(all(route["verb"] for route in routes if route["scope"] != "action_cable"))
        self.assertTrue(all(route["controller"] and route["action"] for route in routes if route["scope"] != "action_cable"))


if __name__ == "__main__":
    unittest.main()
