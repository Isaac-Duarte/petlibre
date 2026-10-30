//! What Dart calls. Each module is one screen flow's needs; the values
//! that cross are owned, so nothing borrows across the bridge.

pub mod error;
pub mod feeder;
pub mod install;
pub mod join;
pub mod keys;
pub mod model;
pub mod restore;
pub mod session;
pub mod setup;
pub mod store;
pub mod update;

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}
