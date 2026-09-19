#!/usr/bin/env python3
"""Golden-wire parse test: every committed tone decodes into its typed
view with the exact wire keys views.rs declares. Drift in views.rs or in
this client's generated mirrors fails here."""

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from thrum import protocol as p  # noqa: E402

FIXTURE = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..",
    "..",
    "..",
    "thrum-core",
    "tests",
    "fixtures",
    "golden.ndjson",
)

ENV_KEYS = {"chi", "rid", "from", "to", "sigil", "sid", "wane", "sentAt", "dusk", "ext"}


def lines():
    with open(FIXTURE) as f:
        return f.read().splitlines()


class GoldenWireTest(unittest.TestCase):
    def test_every_line_is_a_known_tone(self):
        for raw in lines():
            tone = json.loads(raw)
            self.assertIn(tone["chi"], p.TONE_VIEWS, raw)
            self.assertIn("rid", tone)
            self.assertIn("sentAt", tone)

    def test_bodies_decode_through_generated_views(self):
        decoded = 0
        for raw in lines():
            tone = json.loads(raw)
            chi = tone["chi"]
            body = {k: v for k, v in tone.items() if k not in ENV_KEYS}
            view = p.from_wire(p.TONE_VIEWS[chi], body)
            # every wire key must be a declared field of the view
            declared = set(p.TONE_VIEWS[chi]._wire.values())
            unknown = set(body) - declared
            self.assertEqual(unknown, set(), f"{raw}: {unknown}")
            # and the view must re-serialize to exactly this body
            self.assertEqual(p.to_wire(view), body, raw)
            decoded += 1
        self.assertEqual(decoded, 15)

    def test_spot_checks(self):
        tones = {json.loads(r)["chi"]: json.loads(r) for r in lines()}
        hello = p.from_wire(
            p.HelloBody, {k: v for k, v in tones["hello"].items() if k not in ENV_KEYS}
        )
        self.assertEqual(hello.proto_version, "0.7.0")
        self.assertEqual(hello.tool_names, ["Read", "Bash"])
        chunk = p.from_wire(
            p.ChunkBody, {k: v for k, v in tones["chunk"].items() if k not in ENV_KEYS}
        )
        self.assertEqual(chunk.chunk_type, "tool_input_delta")
        self.assertEqual(chunk.block_idx, 2)
        self.assertEqual(chunk.partial_json, '{"a":')
        result = p.from_wire(
            p.ToolResultBody,
            {k: v for k, v in tones["tool-result"].items() if k not in ENV_KEYS},
        )
        self.assertEqual(result.output, "file contents")
        self.assertEqual(result.is_error, False)


if __name__ == "__main__":
    unittest.main()
