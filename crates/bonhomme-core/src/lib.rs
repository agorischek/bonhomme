//! The language-agnostic core of bonhomme: immutable operations, the materialized semantic graph,
//! deterministic replay/validation, merge analysis, and the [`LanguagePlugin`] boundary that a
//! concrete language (TypeScript, etc.) implements. This crate depends on no language and no
//! storage backend.

pub mod core;
pub mod lang;

pub use core::*;
pub use lang::safe_relative_path;
pub use lang::*;
