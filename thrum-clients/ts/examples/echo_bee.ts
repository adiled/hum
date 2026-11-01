// An echo bee: it answers every prompt with its own text. It is the
// smallest whole bee in TS, and the conformance run drives it the same
// way it drives the echo bee in every other language.
//
//   ECHO_HIVE (echo-ts), ECHO_AS (main), ECHO_MODEL (echo-ts)
//
// It prints what happens to it, one line each: nestled, astray,
// advisory <code>, then closed, refused <why> or failed <why>. A prompt
// whose text is "stray" is answered with a key the thrum does not
// declare; one whose text is "follow" has the bee follow its sigil and
// print sid <chi> for the next tone of it. Closing its stdin closes the
// bee.

import { Bee, Chi, PetalKind, Refused, Role } from "../index.ts";

const say = (line: string) => process.stdout.write(line + "\n");
const model = process.env.ECHO_MODEL || "echo-ts";

const bee = new Bee({
  hive: process.env.ECHO_HIVE || "echo-ts",
  as: process.env.ECHO_AS || "main",
  role: Role.nurse,
  hello: { version: "1.0.0", models: [model], headroom: { freeSlots: 1, totalSlots: 1 } },
});

bee.on(Chi.prompt, (tone) => {
  const text = typeof tone.text === "string" ? tone.text : "";
  const sid = tone.sid ?? "";
  if (text === "follow") {
    bee.onSid(sid, (next) => {
      say(`sid ${next.chi}`);
      bee.offSid(sid);
    });
  }
  const unfurl = { chi: Chi.unfurl, sid, petal: 0, kind: PetalKind.text, text: `echo: ${text}` };
  bee.send(text === "stray" ? ({ ...unfurl, stray: true } as typeof unfurl) : unfurl);
  bee.send({ chi: Chi.wilt, sid, reason: "stop" });
});
bee.onNestled(() => say("nestled"));
bee.onAstray(() => say("astray"));
bee.onAdvisory((a) => say(`advisory ${a.code}`));

process.stdin.on("end", () => void bee.close());
process.stdin.resume();

try {
  await bee.start();
  await bee.closed();
  say("closed");
  process.exit(0);
} catch (e) {
  if (e instanceof Refused) {
    say(`refused ${e.reason}`);
    process.exit(3);
  }
  say(`failed ${(e as Error).message}`);
  process.exit(1);
}
