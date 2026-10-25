"""The client end of `hum bee link`, for a bee written in Python.

A Bee runs `hum bee link` and speaks to it over two pipes, one JSON object
per line. The link is hum's own client, in Rust: it enters the bee, signs
the hello, reads humd's verdict, reconnects when humd restarts, and checks
every tone against the thrum. So this module knows no protocol, and a
Python bee behaves exactly as a bee in any language.

    bee = Bee(hive="echo", as_="main", hello={"roles": ["worker"], "models": ["m"]})
    bee.on(Chi.PROMPT, on_prompt)
    bee.on_astray(stop_turns)
    await bee.start()    # admitted, or Refused, or LinkError
    await bee.closed()   # how the link ended; Refused or LinkError unless the bee closed it

A tone goes to the handler for its sid if there is one, else to the
handler for its chi, else to on_any: a forager follows each sigil it
opened by sid, a worker takes prompts by chi. Handlers run in the order
tones arrive, on the event loop. A coroutine handler runs as its own
task, so a long bloom never holds up the tone after it (a cancel). A tone sent while humd is away is dropped by the
link, which says so.
"""
from __future__ import annotations

import asyncio
import json
import os
import shutil
import sys
from typing import Any, Awaitable, Callable, Dict, List, Mapping, Optional, Set, Union

# @constants

Tone = Dict[str, Any]
Handler = Callable[..., Union[None, Awaitable[None]]]

_LINE_LIMIT = 64 * 1024 * 1024


class Refused(Exception):

    def __init__(self, reason: str) -> None:
        super().__init__(f"humd refused the bee: {reason}")
        self.reason = reason


class LinkError(Exception):
    pass


def hum_cli() -> str:
    explicit = os.environ.get(HUM_CLI_ENV)
    if explicit:
        return explicit
    found = shutil.which(HUM_CLI_NAME)
    if found:
        return found
    return os.path.join(os.path.expanduser("~"), HUM_CLI_DEFAULT)


class Bee:
    def __init__(
        self,
        hive: str,
        as_: str,
        hello: Optional[Mapping[str, Any]] = None,
        hum: Optional[str] = None,
    ) -> None:
        self.hive = hive
        self.as_ = as_
        self.id = ""
        self._hello: Dict[str, Any] = dict(hello or {})
        self._hum = hum
        self._by_sid: Dict[str, Handler] = {}
        self._by_chi: Dict[str, Handler] = {}
        self._any: Optional[Handler] = None
        self._nestled: List[Handler] = []
        self._astray: List[Handler] = []
        self._advisory: List[Handler] = []
        self._proc: Optional[asyncio.subprocess.Process] = None
        self._starting: Optional[asyncio.Future] = None
        self._done: Optional[asyncio.Future] = None
        self._closing = False
        self._refusal: Optional[str] = None
        self._failure: Optional[Exception] = None
        self._tasks: Set[asyncio.Future] = set()

    def on(self, chi: str, fn: Handler) -> "Bee":
        self._by_chi[chi] = fn
        return self

    def on_sid(self, sid: str, fn: Handler) -> "Bee":
        self._by_sid[sid] = fn
        return self

    def off_sid(self, sid: str) -> "Bee":
        self._by_sid.pop(sid, None)
        return self

    def on_any(self, fn: Handler) -> "Bee":
        self._any = fn
        return self

    def on_nestled(self, fn: Handler) -> "Bee":
        self._nestled.append(fn)
        return self

    def on_astray(self, fn: Handler) -> "Bee":
        self._astray.append(fn)
        return self

    def on_advisory(self, fn: Handler) -> "Bee":
        self._advisory.append(fn)
        return self

    async def start(self) -> None:
        if self._proc is not None or self._done is not None:
            raise LinkError("this bee has already started")
        loop = asyncio.get_running_loop()
        self._done = loop.create_future()
        self._done.add_done_callback(_retrieve)
        self._starting = loop.create_future()
        self._starting.add_done_callback(_retrieve)
        hum = self._hum or hum_cli()
        try:
            self._proc = await asyncio.create_subprocess_exec(
                hum, "bee", "link", "--hive", self.hive, "--as", self.as_,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                limit=_LINE_LIMIT,
            )
        except OSError as e:
            self._end(LinkError(f"could not run {hum}: {e}"))
        else:
            self._write({"hello": self._hello})
            self._spawn(self._read())
        await asyncio.shield(self._starting)

    def send(self, tone: Mapping[str, Any]) -> None:
        self._write(tone)

    def hello(self, patch: Mapping[str, Any]) -> None:
        self._write({"hello": patch})

    async def close(self) -> None:
        self._closing = True
        if self._proc is not None and self._proc.stdin is not None:
            self._proc.stdin.close()
        elif self._done is None:
            return
        await self.closed()

    async def closed(self) -> None:
        if self._done is None:
            raise LinkError("this bee has not started")
        await asyncio.shield(self._done)

    def _write(self, msg: Any) -> None:
        proc = self._proc
        if proc is None or proc.stdin is None or proc.stdin.is_closing():
            return
        try:
            proc.stdin.write((json.dumps(msg, separators=(",", ":")) + "\n").encode())
        except (BrokenPipeError, ConnectionResetError):
            pass

    async def _read(self) -> None:
        assert self._proc is not None and self._proc.stdout is not None
        while True:
            try:
                raw = await self._proc.stdout.readline()
            except (ValueError, asyncio.LimitOverrunError):
                self._failure = LinkError("a line from the link was over the client's limit")
                self._proc.kill()
                break
            if not raw:
                break
            self._receive(raw)
        self._exited(await self._proc.wait())

    def _receive(self, raw: bytes) -> None:
        try:
            msg = json.loads(raw)
        except ValueError:
            return
        if not isinstance(msg, dict):
            return
        chi = msg.get("chi")
        if isinstance(chi, str):
            sid = msg.get("sid")
            fn = (self._by_sid.get(sid) if isinstance(sid, str) else None) or self._by_chi.get(chi) or self._any
            if fn is not None:
                self._call(fn, msg)
            return
        link = msg.get("link")
        if link == "ready":
            if msg.get("linkVersion") != LINK_VERSION:
                self._failure = LinkError(
                    f"hum speaks link version {msg.get('linkVersion')} and this client "
                    f"{LINK_VERSION}; rebuild the hive against the installed hum"
                )
                if self._proc is not None:
                    self._proc.kill()
                return
            self.id = str(msg.get("bee") or "")
        elif link == "nestled":
            if self._starting is not None and not self._starting.done():
                self._starting.set_result(None)
            for fn in self._nestled:
                self._call(fn)
        elif link == "astray":
            for fn in self._astray:
                self._call(fn)
        elif link == "advisory":
            advisory = {k: v for k, v in msg.items() if k != "link"}
            for fn in self._advisory:
                self._call(fn, advisory)
        elif link == "refused":
            self._refusal = str(msg.get("error") or "refused")

    def _exited(self, code: int) -> None:
        if code == LinkExit.REFUSED:
            self._end(Refused(self._refusal or "refused"))
        elif self._failure is not None:
            self._end(self._failure)
        elif code == LinkExit.CLOSED and self._closing:
            self._end(None)
        else:
            how = f"was killed by signal {-code}" if code < 0 else f"exited {code}"
            usage = ", a usage error" if code == LinkExit.USAGE else ""
            self._end(LinkError(f"the link {how}{usage}"))

    def _end(self, err: Optional[Exception]) -> None:
        if self._starting is not None and not self._starting.done():
            self._starting.set_exception(err or LinkError("the link closed before humd admitted the bee"))
        if self._done is not None and not self._done.done():
            if err is None:
                self._done.set_result(None)
            else:
                self._done.set_exception(err)

    def _call(self, fn: Handler, *args: Any) -> None:
        try:
            result = fn(*args)
        except Exception as e:
            print(f"thrum: a handler failed: {e!r}", file=sys.stderr)
            return
        if asyncio.iscoroutine(result) or isinstance(result, asyncio.Future):
            self._spawn(result)

    def _spawn(self, work: Any) -> None:
        task = asyncio.ensure_future(work)
        self._tasks.add(task)
        task.add_done_callback(self._finished)

    def _finished(self, task: asyncio.Future) -> None:
        self._tasks.discard(task)
        if not task.cancelled() and task.exception() is not None:
            print(f"thrum: a handler failed: {task.exception()!r}", file=sys.stderr)


def _retrieve(future: asyncio.Future) -> None:
    if not future.cancelled():
        future.exception()
