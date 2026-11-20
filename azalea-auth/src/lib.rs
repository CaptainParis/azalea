#![doc = include_str!("../README.md")]

#[cfg(feature = "online-mode")]
mod auth;
#[cfg(feature = "online-mode")]
pub mod cache;
#[cfg(feature = "online-mode")]
pub mod certs;
pub mod endpoints;
#[cfg(feature = "online-mode")]
pub mod sessionserver;
#[cfg(feature = "online-mode")]
pub mod steps;
#[cfg(feature = "online-mode")]
pub use auth::*;

pub mod game_profile;
