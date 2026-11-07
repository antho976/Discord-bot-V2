//! The Idleon account check's reading of a save, with no server in it: the save, the game's
//! own data, every mechanic's check, the tiers and the review, and the words every screen shows
//! for them (`view`). The bot's web page and the desktop app both build on this.

pub mod checks;
pub mod crowd;
pub mod expected;
pub mod game;
pub mod pictures;
pub mod review;
pub mod save;
pub mod view;

pub use checks::{all as run_checks, world_reached};
pub use pictures::has as has_picture;
