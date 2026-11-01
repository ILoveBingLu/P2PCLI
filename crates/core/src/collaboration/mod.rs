//! Durable, explicitly authorized collaboration between device-local AI sessions.

pub mod model;
pub mod store;
pub mod delivery;
pub mod inbox;

pub use model::*;
pub use store::Store;
