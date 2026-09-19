#!/usr/bin/env python3
"""rid() parity vectors against hum_identity's encoder, taken verbatim
from thrum-core/../hum-identity/src/ids.rs ts_parity_vectors."""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from thrum.helpers import _hum_id_from_bytes, is_valid_rid, rid  # noqa: E402


class RidParityTest(unittest.TestCase):
    def test_vectors_match_rust(self):
        self.assertEqual(_hum_id_from_bytes(bytes(32)), "0" * 52)
        self.assertEqual(
            _hum_id_from_bytes((1700000000000).to_bytes(6, "big") + bytes(26)),
            "065WZSB8" + "0" * 44,
        )
        self.assertEqual(_hum_id_from_bytes(bytes([0xFF] * 32)), "Z" * 51 + "G")

    def test_rid_shape(self):
        for _ in range(100):
            id_ = rid()
            self.assertTrue(is_valid_rid(id_), id_)
            self.assertEqual(len(id_), 52)

    def test_is_valid_rid(self):
        self.assertFalse(is_valid_rid(""))
        self.assertFalse(is_valid_rid("I" * 52))
        self.assertFalse(is_valid_rid("0" * 51))
        self.assertTrue(is_valid_rid(rid()))


if __name__ == "__main__":
    unittest.main()
