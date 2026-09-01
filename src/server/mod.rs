pub mod api;
pub mod control_api;
pub mod folder_api;
pub mod folder_scan;
pub mod services_api;
pub mod static_files;
pub mod ws;

use crate::collector::snapshot::SystemSnapshot;
use crate::server::folder_scan::ScanCoordinator;
use crate::services::ManifestSource;
use crate::storage::db::MetricsDb;
use crate::storage::ring_buffer::RingBuffer;
use crate::system_info::SystemInfo;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

pub struct AppState {
    pub ring_buffer: Arc<RwLock<RingBuffer>>,
    pub system_info: SystemInfo,
    pub broadcast_tx: broadcast::Sender<SystemSnapshot>,
    pub db: Arc<MetricsDb>,
    /// Serialises the three folder-scan triggers.
    pub folder_scan: Arc<ScanCoordinator>,
    /// The declared service list, reloaded when the file on disk changes.
    pub services: Arc<ManifestSource>,
    /// Root-owned wrapper that performs privileged service control. Kept as
    /// configuration because webtop is a general tool and must not hardcode
    /// one stack's layout.
    pub control_helper: String,
}

impl AppState {
    pub fn new(
        system_info: SystemInfo,
        db: Arc<MetricsDb>,
        manifest_path: PathBuf,
        control_helper: String,
    ) -> Arc<Self> {
        let (broadcast_tx, _) = broadcast::channel(16);
        Arc::new(Self {
            ring_buffer: Arc::new(RwLock::new(RingBuffer::new(3600))),
            system_info,
            broadcast_tx,
            db,
            folder_scan: Arc::new(ScanCoordinator::default()),
            services: Arc::new(ManifestSource::new(manifest_path)),
            control_helper,
        })
    }
}
