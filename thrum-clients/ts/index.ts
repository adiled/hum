// Thrum — TS surface of the hum protocol.
//
// All three generated files come from thrum-core (the Rust source):
//   - chi.ts       — Chi enum, PulseKind, validators
//   - helpers.ts   — sigil, rid, duskIn, isDusk, WaneTracker
//   - protocol.ts  — per-chi body views + the definitive Envelope
//
// Hand-edit the Rust source, then regen (`cargo run -p codegen`).
// This index.ts is a thin barrel — every export here flows through
// from one of the three generated files.

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

export type { ChiKind, PulseKindT, Tone } from "./chi.ts";

export {
  sigil,
  rid,
  duskIn,
  isDusk,
  WaneTracker,
} from "./helpers.ts";

// Protocol views — per-chi body types, ToneViews, plus the definitive
// wire Envelope. Prefer this Envelope over chi.ts's validator-local copy.
export * from "./protocol.ts";
