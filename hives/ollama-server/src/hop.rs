//! The mark the nurse puts on its calls to Ollama.
//!
//! The forager speaks Ollama's API, so a nurse whose `OLLAMA_NURSE_URL`
//! points at it would prompt hum, which would hand the prompt back to the
//! nurse. The nurse marks every call with [`FROM_NURSE`], and the forager
//! refuses a marked call.

pub const FROM_NURSE: &str = "x-hum-from-nurse";
