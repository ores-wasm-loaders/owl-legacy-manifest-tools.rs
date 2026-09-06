//! owl-manifest-tools — read a build tree, write the release manifest, verify one.
//!
//! Standard library only, on purpose: this runs in every product org's CI before any
//! dependency is installed, and a generator that cannot start is a generator that is skipped.
pub mod json;
pub mod manifest;
pub mod scan;
pub mod sha256;
