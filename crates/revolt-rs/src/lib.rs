//! revolt-rs: revolt/event-loop implemented natively in Rust, for PHP 8.2-8.6.
//!
//! - `zend`: the Zend Engine layer on top of ext-php-rs's raw bindings.
//! - `core`: the event loop (records, queues, ticks, the callback fiber, the reactor).
//! - `classes`: the PHP classes of revolt/event-loop 1.0.9.
//! - `module`: the hooks a host extension calls from its module entry (MINIT, RINIT, ...): the
//!   standalone `revolt` extension, or any extension embedding revolt-rs (see `embed`).
//! - `embed`: the native API for embedders, and the C ABI that finds the owning copy.
#![allow(clippy::missing_safety_doc)]
// PHP's ZEND_FASTCALL functions (internal function handlers among them) are __vectorcall on
// Windows; ext-php-rs needs the same (nightly-only) ABI there
#![cfg_attr(windows, feature(abi_vectorcall))]

pub mod classes;
pub mod core;
pub mod embed;
pub mod module;
pub mod zend;
