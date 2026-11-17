//! A stand-in for the `ztracing` crate of the Zed repository, which is licensed
//! GPL-3.0-or-later and would make the combined `spooky` binary GPL.
//!
//! GPUI reaches `ztracing` only through `sum_tree`, which uses one item of it: the
//! `#[instrument(skip_all)]` attribute, to mark functions for the Tracy profiler. This
//! crate provides that attribute as a no-op, so the functions run unchanged and
//! `spooky` never has a profiler attached. It is wired in with a `[patch]` entry in the
//! workspace `Cargo.toml`; see `docs/architecture.md`.

use proc_macro::TokenStream;

/// Marks a function for profiling in the original crate; here it leaves it as written.
#[proc_macro_attribute]
pub fn instrument(_arguments: TokenStream, function: TokenStream) -> TokenStream {
    function
}
