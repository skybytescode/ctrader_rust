//! UI systems for Bevy UI
//!
//! Organized by responsibility:
//! - layout: Sidebar visibility, scroll state calculations
//! - interactions: Button clicks, drags, hover detection
//! - price_updates: Bid/ask text and color updates
//! - virtualization: Spawn/despawn visible instrument rows

pub mod layout;
pub mod interactions;
pub mod price_updates;
pub mod virtualization;

pub use layout::*;
pub use interactions::*;
pub use price_updates::*;
pub use virtualization::*;
