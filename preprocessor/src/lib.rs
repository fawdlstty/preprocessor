//! # preprocessor
//!
//! Compile-time computation macros for Rust. Analyzes code for computable
//! sub-expressions and evaluates them at compile time, so the final binary
//! contains only the results.
//!
//! ## Macros
//!
//! | Macro | Scope | Description |
//! |---|---|---|
//! | `#[preprocessor::optimize]` | Function | Optimizes all evaluable expressions in a function body |
//! | `preprocessor::op!(...)` | Expression | Evaluates a single expression at compile time |
//!
//! ## Features
//!
//! | Feature | Description |
//! |---|---|
//! | `disabled` | Disables all compile-time optimization; macros become transparent passthrough |
//!
//! ## Example
//!
//! ```rust
//! use preprocessor::op;
//!
//! // Compile-time evaluation
//! let result = op!(1 + 2 * 3); // → 7
//!
//! // With free variables — passed through
//! let x = 5;
//! let y = op!(x + 1); // → x + 1 (unchanged)
//! ```
//!
//! ```rust,ignore
//! use preprocessor::optimize;
//!
//! #[optimize]
//! fn compute() -> i32 {
//!     let a = 1 + 2; // → 3
//!     let b = 4 * 5; // → 20
//!     a + b
//! }
//! ```

#[cfg(not(feature = "disabled"))]
pub use preprocessor_derive::{op, optimize, prelude};

/// When `disabled` feature is enabled, `op!` becomes transparent passthrough.
#[cfg(feature = "disabled")]
#[macro_export]
macro_rules! op {
    ($($tt:tt)*) => {
        $($tt)*
    };
}

/// When `disabled` feature is enabled, `#[optimize]` becomes a no-op passthrough.
/// Uses a declarative macro wrapper that re-emits the function unchanged.
#[cfg(feature = "disabled")]
pub use preprocessor_derive::optimize;
