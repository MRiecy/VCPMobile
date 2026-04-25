pub mod window;

use dashmap::DashMap;

pub struct PortalState {
    pub contents: DashMap<String, String>,
}

impl Default for PortalState {
    fn default() -> Self {
        Self::new()
    }
}

impl PortalState {
    pub fn new() -> Self {
        Self {
            contents: DashMap::new(),
        }
    }
}
