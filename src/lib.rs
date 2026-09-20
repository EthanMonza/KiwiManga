//! KiwiManga — personal Telegram bot for downloading manga & ranobe.
//!
//! Thin handlers enqueue jobs into SQLite; worker tasks do all downloading,
//! packing and uploading. See README.md for architecture details.

// Doc comments mention product/API names (CBZ, MangaDex, SQLite, BotFather, ...);
// wrapping every one in backticks hurts more than it helps.
#![allow(clippy::doc_markdown)]

pub mod bot;
pub mod config;
pub mod db;
pub mod error;
pub mod http;
pub mod i18n;
pub mod langdetect;
pub mod normalize;
pub mod pack;
pub mod queue;
pub mod ratelimit;
pub mod sources;
pub mod state;
pub mod worker;
