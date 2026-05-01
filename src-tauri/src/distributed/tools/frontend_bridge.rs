// distributed/tools/frontend_bridge.rs
// Shared data store for Phase 2 frontend-cooperative sensors.
// Frontend (Vue) collects sensor data via Web APIs and pushes to Rust via Tauri command.
// Each Phase 2 StreamingTool reads from this shared store.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// Global sensor data store. Keys are sensor identifiers (e.g. "location", "motion", "ambient").
/// Values are the latest formatted string from the frontend.
static SENSOR_DATA: LazyLock<RwLock<HashMap<String, SensorEntry>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

struct SensorEntry {
    value: String,
    updated_at: std::time::Instant,
}

/// Called by the Tauri command to update sensor data from frontend.
pub fn update_sensor(key: &str, value: String) {
    if let Ok(mut map) = SENSOR_DATA.write() {
        map.insert(
            key.to_string(),
            SensorEntry {
                value,
                updated_at: std::time::Instant::now(),
            },
        );
    }
}

/// Read the latest value for a sensor key.
/// Returns None if no data has been pushed yet or if data is stale (> max_age).
pub fn read_sensor(key: &str, max_age_secs: u64) -> Option<String> {
    if let Ok(map) = SENSOR_DATA.read() {
        if let Some(entry) = map.get(key) {
            if entry.updated_at.elapsed().as_secs() <= max_age_secs {
                return Some(entry.value.clone());
            }
        }
    }
    None
}
