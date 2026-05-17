"""thrum — wire-protocol primitives for Python nestlings.

See WIRE.md for the protocol spec. This package re-exports:

- Chi, PulseKind, THRUM_VERSION, ALL_CHI, is_valid_chi (generated)
- sigil, rid, dusk_in, is_dusk, WaneTracker (generated)
- ThrumClient (hand-written socket client)
"""
from .chi import (
    ALL_CHI,
    Chi,
    PulseKind,
    THRUM_VERSION,
    is_valid_chi,
)
from .client import ThrumClient, Tone
from .helpers import (
    WaneTracker,
    default_socket_path,
    dusk_in,
    is_dusk,
    now_ms,
    rid,
    sigil,
)

__all__ = [
    "ALL_CHI",
    "Chi",
    "PulseKind",
    "THRUM_VERSION",
    "ThrumClient",
    "Tone",
    "WaneTracker",
    "default_socket_path",
    "dusk_in",
    "is_dusk",
    "is_valid_chi",
    "now_ms",
    "rid",
    "sigil",
]
