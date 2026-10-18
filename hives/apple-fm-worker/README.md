# apple-fm-worker

A WorkerBee that runs **Apple's on-device 3B FoundationModel** (via the
`FoundationModels` framework) as a local, network-free, key-free LLM — a
sibling of `ollama-worker`.

It registers with `humd` over thrum and advertises the model
`apple-3b-on-device`, turning `chi:"prompt"` into `chunk`/`finish` tones,
exactly like ollama-worker does for a remote Ollama endpoint.

## Architecture

The Rust side (`src/`) is a thin `WorkerBee` — it mirrors `ollama-worker`
but, instead of issuing HTTP to Ollama, it **shells out to a small Swift
binary** (`bridge/gen`) that wraps `FoundationModels`. The bridge reads a
JSON line `{system, prompt}` on stdin and emits NDJSON on stdout:

    {"event":"delta","text":"…"}   (one per streaming token)
    {"event":"finish","finishReason":"stop"}

The Rust worker diffs the cumulative snapshots `streamResponse` returns and
forwards the per-token deltas as `chunk` tones.

The **only** Apple-specific code in the whole tree is `bridge/gen.swift`
(+ `probe.swift`), compiled with `swiftc`. No Apple symbols enter the Rust
side. The bridge is `#[cfg(target_os = "macos")]`-gated; non-mac builds of
hum never reference it.

## Capability gate

On boot the worker runs `gen --probe`. If the on-device model is not
available — Apple Intelligence off, asset not downloaded, unsupported
region — the probe exits non-zero and the hive stays **present-but-dormant**
instead of advertising a live worker, mirroring how ollama-worker degrades
when Ollama isn't running.

## Build

    cd hives/apple-fm-worker/bridge && ./build.sh   # swiftc -O -> ./gen
    cd .. && cargo build --release                   # -> target/release/apple-fm-worker

Requires macOS 26+ with Apple Intelligence enabled
(System Settings → Apple Intelligence & Siri).

## Status

Isolated prototype. Verified on this host:
- `gen --probe` → `{"event":"probe","available":true}`
- `gen` with a prompt → real on-device deltas + `finish`
- the Rust worker compiles, connects to a live `humd`, and hands its
  `hello` advertising `apple-3b-on-device`.

Not yet co-installed into the mac desktop bundle — pending an end-to-end
prompt routed through `humd` to the worker and back.
