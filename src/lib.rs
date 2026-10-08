//! Typed conversion for coding-agent session transcripts.
//!
//! Claude Code, Claude Chat, Cowork, Codex, `OpenCode`, pi, Campfire, Cursor,
//! Grok, Hermes, Amp, and Antigravity record similar conversation data in
//! different stores. This crate maps each format through [`Transcript<Common>`] and
//! converts with [`convert::<A, B>`](convert): `A` -> [`Common`] -> `B`.
//!
//! # Fidelity
//!
//! Stores preserve native disk shape. [`Common`] preserves resumable
//! conversation semantics, not byte identity.
//!
//! # Shape
//!
//! - [`common`] — the canonical model ([`common::Message`], [`common::Block`],
//!   [`common::Tool`], …).
//! - [`jev`] — the handoff context-planning layer (advisory decisions over
//!   the canonical model; changes nothing on its own).
//! - [`Transcript`], [`Harness`], [`Codec`], [`Store`] — the generic type and
//!   the traits over it.
//! - [`harness`] — one module per implemented harness.

pub mod common;
pub mod error;
pub mod harness;
pub mod jev;
#[cfg(all(feature = "jev_api", not(target_arch = "wasm32")))]
pub mod jev_api;
#[cfg(not(target_arch = "wasm32"))]
pub mod local;
#[cfg(all(feature = "search", not(target_arch = "wasm32")))]
pub mod retrieval;
#[cfg(feature = "search")]
pub mod search;
pub mod text;
mod transcript;

#[cfg(feature = "wasm")]
mod wasm;

// The core generic API lives in the private `transcript` module, so the crate
// root is its canonical home. The concrete model and per-harness types keep
// their own module homes — reach them through [`common`] and [`harness`]
// rather than flattened at the root.
pub use error::{Error, Result};
pub use transcript::{
    Codec, Common, CropError, Discovered, Harness, HarnessId, Saved, Span, Store, TextCodec,
    Transcript, convert,
};
