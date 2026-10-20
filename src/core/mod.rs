//! Core domain logic, deliberately free of any terminal/UI dependency so that it
//! can be unit tested without a TTY.

pub mod backup;
pub mod config;
pub mod database;
pub mod migrate;
pub mod paths;
pub mod processes;
pub mod scan;
pub mod sessions;
pub mod types;
