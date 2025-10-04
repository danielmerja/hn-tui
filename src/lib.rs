#![allow(clippy::uninlined_format_args)]

pub mod app;
pub mod auth;
pub mod config;
pub mod data;
pub mod markdown;
pub mod media;
pub mod reddit;
pub mod session;
pub mod storage;
pub mod ui;

pub use app::run;
