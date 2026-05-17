"""Async ThrumClient — connect to humd's NDJSON socket, send/receive tones.

Spec: WIRE.md. ~120 LoC. No magic — open Unix stream, write JSON+`\\n`,
read JSON+`\\n`, dispatch by sid.

Usage:

    import asyncio
    from thrum import Chi, ThrumClient, rid, THRUM_VERSION

    async def main() -> None:
        c = ThrumClient()
        await c.connect()
        await c.send({
            "chi": Chi.HELLO,
            "rid": rid(),
            "from": "my-nestling",
            "nestling": "my-nestling",
            "version": "0.1.0",
            "protoVersion": THRUM_VERSION,
        })

        async def on_sid_x(tone):
            print("got tone for sid x:", tone)
        c.on("hum-x", on_sid_x)
        c.on_any(lambda t: print("catch-all:", t.get("chi")))

        await c.run_forever()

    asyncio.run(main())
"""
from __future__ import annotations

import asyncio
import json
from typing import Any, Awaitable, Callable, Dict, Mapping, Optional, Union

from .helpers import default_socket_path

Tone = Dict[str, Any]
Handler = Callable[[Tone], Union[None, Awaitable[None]]]


class ThrumClient:
    """Minimal NDJSON client for humd's thrum socket.

    Async-only. One client per logical nestler connection.
    """

    def __init__(self, socket_path: Optional[str] = None) -> None:
        self._path = socket_path or default_socket_path()
        self._reader: Optional[asyncio.StreamReader] = None
        self._writer: Optional[asyncio.StreamWriter] = None
        self._handlers: Dict[str, Handler] = {}
        self._wildcard: Optional[Handler] = None
        self._connected = asyncio.Event()
        self._pending: list[bytes] = []
        self._lock = asyncio.Lock()

    @property
    def socket_path(self) -> str:
        return self._path

    async def connect(self) -> None:
        """Open the Unix socket. Buffered writes flush after this returns."""
        if self._writer is not None:
            return
        reader, writer = await asyncio.open_unix_connection(self._path)
        self._reader = reader
        self._writer = writer
        self._connected.set()
        # Flush any sends that happened before connect resolved.
        for line in self._pending:
            writer.write(line)
        self._pending = []
        await writer.drain()

    async def send(self, tone: Mapping[str, Any]) -> None:
        """Serialize `tone` as NDJSON and write to the socket.

        If called before `connect()`, the write is buffered.
        """
        line = (json.dumps(tone, separators=(",", ":")) + "\n").encode("utf-8")
        async with self._lock:
            if self._writer is None:
                self._pending.append(line)
                return
            self._writer.write(line)
            try:
                await self._writer.drain()
            except (BrokenPipeError, ConnectionResetError):
                pass

    def on(self, sid: str, handler: Handler) -> None:
        """Register a handler for tones with the given sid."""
        self._handlers[sid] = handler

    def off(self, sid: str) -> None:
        self._handlers.pop(sid, None)

    def on_any(self, handler: Handler) -> None:
        """Catch-all for tones without a sid OR with an unregistered sid."""
        self._wildcard = handler

    async def run_forever(self) -> None:
        """Read frames until the socket closes. Dispatches each tone to its
        registered handler (or the wildcard). Returns when EOF is reached."""
        if self._reader is None:
            await self.connect()
        assert self._reader is not None
        while True:
            try:
                raw = await self._reader.readline()
            except (asyncio.IncompleteReadError, ConnectionResetError):
                break
            if not raw:
                break
            line = raw.decode("utf-8", errors="replace").rstrip("\n")
            if not line:
                continue
            try:
                tone: Tone = json.loads(line)
            except json.JSONDecodeError:
                continue
            sid = tone.get("sid")
            handler = (
                self._handlers.get(sid) if isinstance(sid, str) else None
            ) or self._wildcard
            if handler is None:
                continue
            try:
                result = handler(tone)
                if asyncio.iscoroutine(result):
                    await result
            except Exception:
                # Handlers are user code; never let one kill the read loop.
                continue

    async def close(self) -> None:
        if self._writer is not None:
            try:
                self._writer.close()
                await self._writer.wait_closed()
            except (BrokenPipeError, ConnectionResetError):
                pass
        self._writer = None
        self._reader = None
        self._connected.clear()
