# thrum

> _wire-protocol primitives for thrum, the NDJSON socket between humd and nestlings_

`thrum` is the TypeScript contract for the **thrum** protocol that
nestlings (clients) and humds (daemons) speak to each other. Importing
this package is how you build a TS / JS nestling without copy-pasting
chi constants.

It's generated from the Rust source of truth (`thrum-core`) — bumping a
chi in Rust regenerates `chi.ts` on the next `cargo build`.

## Install

```bash
# Once published to npm:
npm install thrum

# Today (git dep with subdirectory):
npm install github:adiled/hum#main --workspaces=false
# or in package.json:
#   "thrum": "git+https://github.com/adiled/hum.git#main"
```

## What's in it

```ts
import {
  Chi,            // const map of every chi value
  ALL_CHI,        // string array, useful for switch-exhaustiveness
  PulseKind,      // pulse.kind enum
  THRUM_VERSION,  // "0.7.0" today; bump when chi.rs changes
  sigil,          // sha256(nest, sid)[..12] hex
  rid,            // monotonic request id
  WaneTracker,    // Lamport-clock per sigil
} from "thrum";

import type { ChiKind, Tone, Envelope } from "thrum";
```

## Build a nestling

```ts
import { Chi, THRUM_VERSION } from "thrum";
import { createConnection } from "node:net";
import { homedir } from "node:os";
import { join } from "node:path";

const sockPath = process.env.HUM_THRUM_SOCK ??
  join(process.env.XDG_RUNTIME_DIR ?? `/run/user/${process.getuid?.() ?? 1000}`,
       "hum/thrum.sock");

const sock = createConnection(sockPath);
let buf = "";

sock.on("connect", () => {
  // Handshake. `chi`, `propensity`, and `source` feed the on-mesh
  // nestling registry — see ensemble/README.md.
  sock.write(JSON.stringify({
    chi: Chi.hello,
    rid: `hello-${Date.now().toString(36)}`,
    from: "my-nestling",
    nestling: "my-nestling",
    version: "0.1.0",
    protoVersion: THRUM_VERSION,
    chis: [Chi.hello, Chi.prompt, Chi.chunk, Chi.finish],
  }) + "\n");
});

sock.on("data", (chunk: Buffer) => {
  buf += chunk.toString();
  let nl: number;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line) continue;
    const tone = JSON.parse(line);
    switch (tone.chi) {
      case Chi.breath: /* daemon ack of handshake */ break;
      case Chi.prompt: /* a turn arrived */ break;
    }
  }
});
```

## Version policy

`THRUM_VERSION` is the wire-protocol version and is independent of the
package version. Nestlings warn on mismatch.

| change | THRUM_VERSION bump |
|---|---|
| docstring tweaks, additive-optional fields | patch |
| new chi value, new required field with compat path | minor |
| removed chi, renamed chi, semantics changed | major |

## See also

- [`thrum-core`](../thrum-core) — the Rust source of truth this package
  is generated from.
- [`ensemble`](../ensemble) — the mesh of humds. Nestlings advertise
  themselves and discover each other on the
  `hum/nestlings/announce` gossip topic.
- [`nestlings/`](../nestlings) — reference implementations:
  `openai-server`, `vercel-ai`, `opencode`, `grpc`.
- [adiled.github.io/hum/](https://adiled.github.io/hum/) — docs site.
