//! Process Corral engine library: shared by the `process-corral` service/console binary and the
//! optional `process-corral-gui` binary (feature `gui`). Nothing here depends on any GUI crate.

pub mod apply;
pub mod config;
pub mod elevate;
pub mod engine;
#[cfg(test)]
mod engine_profiles_tests;
#[cfg(all(test, windows))]
mod real_os_tests;
pub mod fsutil;
pub mod guid;
#[cfg(windows)]
pub mod governor;
#[cfg(windows)]
pub mod instance;
pub mod log;
pub mod logview;
pub mod os;
pub mod paths;
pub mod profile;
pub mod profile_edit;
pub mod profiles;
#[doc(hidden)]
pub mod simos;
pub mod reload;
#[cfg(windows)]
pub mod scm;
#[cfg(windows)]
pub mod service;
pub mod settings;
pub mod status;
pub mod topology;
pub mod statusview;
