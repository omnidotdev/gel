//! Core library for gel: the state model, diff planner, and apply engine
//!
//! This crate is intentionally pure and free of system/OS side effects. The
//! package backend is an abstraction; real OS-touching implementations live
//! outside this unit, and tests drive an in-memory fake.

pub mod apply;
pub mod backend;
pub mod error;
pub mod import;
pub mod journal;
pub mod plan;
pub mod snapshot;
pub mod state;

// Real OS-touching implementations, compiled only with the `arch` feature. The
// default build enables none of this and remains pure.
#[cfg(feature = "arch")]
pub mod sys;

pub use backend::PackageBackend;
pub use error::GelError;
