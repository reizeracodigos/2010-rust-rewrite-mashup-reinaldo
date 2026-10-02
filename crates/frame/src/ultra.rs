use bevy::prelude::*;

/// ULTRAKILL movement mode for the local player, toggled with K. While it is
/// on, every command carries `buttons::ULTRA` and pmove moves the player as V1.
#[derive(Resource, Default)]
pub struct UltraMode {
    pub active: bool,
}
