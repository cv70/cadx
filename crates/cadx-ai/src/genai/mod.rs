//! rust-genai adapter for the provider-neutral CADX AI planning contract.
//!
//! The submodules separate provider client construction, the bounded document
//! projection, the system prompt payload, the JSON tool schema, and provider
//! error normalization. This root only wires them together and re-exports the
//! adapter.

mod client;
mod document_view;
mod error;
mod prompt;
mod schema;

pub use client::GenAiAssistant;
