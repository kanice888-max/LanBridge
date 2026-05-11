/// UDP LAN discovery.
///
/// P0: Stub only — manual IP connection is the primary method.
/// P1: Implement UDP broadcast discovery.
pub struct DiscoveryService {
    enabled: bool,
}

impl DiscoveryService {
    pub fn new() -> Self {
        Self { enabled: false }
    }

    /// Start discovery service (P1 feature, disabled in P0).
    pub fn start(&mut self) {
        tracing::info!("UDP discovery is disabled in P0, using manual IP only");
        self.enabled = false;
    }

    /// Stop discovery service.
    pub fn stop(&mut self) {
        self.enabled = false;
    }

    /// Check if discovery is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}
