// Thrum — TS surface of the hum protocol.
//
// Both files in this directory are generated from thrum-core (the Rust
// source of truth):
//   - chi.ts       — Chi enum, PulseKind, Envelope, validators
//   - helpers.ts   — sigil, rid, duskIn, isDusk, WaneTracker
//
// Hand-edit chi.rs (or extend codegen for new helpers). cargo build
// regenerates both files via thrum-core/build.rs.
//
// This index.ts is a thin barrel — every export here flows through
// from one of the two generated files. New protocol primitives should
// be added in Rust first, then surfaced via codegen.

export {
  // Registry + version
  Chi,
  ALL_CHI,
  isValidChi,
  PulseKind,
  THRUM_VERSION,
  // Validators
  isEnvelope,
  isKnownTone,
} from "./chi.ts";

export type {
  ChiKind,
  PulseKindT,
  Envelope,
  Tone,
} from "./chi.ts";

export {
  sigil,
  rid,
  duskIn,
  isDusk,
  WaneTracker,
} from "./helpers.ts";
