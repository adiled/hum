"""An echo bee: it answers every prompt with its own text. It is the
smallest whole bee in Python, and the conformance run drives it the same
way it drives the echo bee in every other language.

    ECHO_HIVE (echo-py), ECHO_AS (main), ECHO_MODEL (echo-py)

It prints what happens to it, one line each: nestled, astray,
advisory <code>, then closed, refused <why> or failed <why>. A prompt
whose text is "stray" is answered with a key the thrum does not declare;
one whose text is "follow" has the bee follow its sigil and print
sid <chi> for the next tone of it. Closing its stdin closes the bee.
"""
import asyncio
import os
import sys
import threading

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from thrum import Bee, Chi, LinkError, PetalKind, Refused, Role


def say(line: str) -> None:
    print(line, flush=True)


async def main() -> int:
    model = os.environ.get("ECHO_MODEL") or "echo-py"
    bee = Bee(
        hive=os.environ.get("ECHO_HIVE") or "echo-py",
        as_=os.environ.get("ECHO_AS") or "main",
        role=Role.NURSE,
        hello={"version": "1.0.0", "models": [model],
               "headroom": {"freeSlots": 1, "totalSlots": 1}},
    )

    def on_prompt(tone):
        text = tone.get("text") if isinstance(tone.get("text"), str) else ""
        sid = tone.get("sid", "")
        if text == "follow":
            def on_next(nxt, sid=sid):
                say(f"sid {nxt.get('chi')}")
                bee.off_sid(sid)
            bee.on_sid(sid, on_next)
        unfurl = {"chi": Chi.UNFURL, "sid": sid, "petal": 0, "kind": PetalKind.TEXT, "text": f"echo: {text}"}
        if text == "stray":
            unfurl["stray"] = True
        bee.send(unfurl)
        bee.send({"chi": Chi.WILT, "sid": sid, "reason": "stop"})

    bee.on(Chi.PROMPT, on_prompt)
    bee.on_nestled(lambda: say("nestled"))
    bee.on_astray(lambda: say("astray"))
    bee.on_advisory(lambda a: say(f"advisory {a['code']}"))

    loop = asyncio.get_running_loop()
    eof = loop.create_future()

    def read_stdin() -> None:
        sys.stdin.read()
        loop.call_soon_threadsafe(lambda: eof.done() or eof.set_result(None))

    threading.Thread(target=read_stdin, daemon=True).start()

    async def close_on_eof() -> None:
        await eof
        await bee.close()

    try:
        await bee.start()
        asyncio.ensure_future(close_on_eof())
        await bee.closed()
    except Refused as e:
        say(f"refused {e.reason}")
        return 3
    except LinkError as e:
        say(f"failed {e}")
        return 1
    say("closed")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
