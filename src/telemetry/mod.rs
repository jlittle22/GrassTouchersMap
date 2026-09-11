/// Telemetry and update-checks against the upstream Turun project's own server have been
/// disabled in this fork. These are kept as no-ops so callers don't need to change.
use crate::view::preferences;

pub fn get_latest_version() {}

pub fn event_load_server(_telemetry_preferences: preferences::Telemetry, _server_id: &str) {}

pub fn event_stored_config(_telemetry_preferences: preferences::Telemetry, _yaml_string: &str) {}
