use std::sync::{Arc, RwLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Playing,
    Paused,
}

#[derive(Clone, Default)]
pub struct Metadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_art: Option<Arc<Vec<u8>>>,
    pub length_seconds: Option<u32>,
    pub seek_position: Option<f64>,
    pub play_state: Option<PlayState>,
    pub track: u32,
    pub tracks_total: u32,
}

/// Thread-safe shared metadata, read-optimised via RwLock<Arc<Metadata>>.
///
/// Readers get a cheap Arc::clone (atomic refcount bump) instead of
/// cloning strings and cover art on every audio frame.  Writers are
/// rare (track changes, seek updates) and pay the full clone cost.
#[derive(Clone)]
pub struct SharedMetadata {
    inner: Arc<RwLock<Arc<Metadata>>>,
    zones: Arc<RwLock<Vec<String>>>,
}

impl SharedMetadata {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Arc::new(Metadata::default()))),
            zones: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn get(&self) -> Arc<Metadata> {
        Arc::clone(&self.inner.read().unwrap())
    }

    pub fn set(&self, meta: Metadata) {
        *self.inner.write().unwrap() = Arc::new(meta);
    }

    pub fn get_zones(&self) -> Vec<String> {
        self.zones.read().unwrap().clone()
    }

    pub fn set_zones(&self, zones: Vec<String>) {
        *self.zones.write().unwrap() = zones;
    }
}
