//! The mark the nurse puts on its calls to Ollama.
//!
//! The forager speaks Ollama's API on Ollama's own port, and the nurse
//! calls Ollama there unless told otherwise, so on a host with no Ollama
//! of its own the nurse would be calling its own forager, which would
//! prompt hum, which would hand the prompt back to the nurse. The nurse
//! marks every call with [`FROM_NURSE`], and the forager refuses a marked
//! call.

pub const FROM_NURSE: &str = "x-hum-from-nurse";
