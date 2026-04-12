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

/// Thread-safe shared metadata, read-optimised via RwLock.
///
/// Cover art uses Arc<Vec<u8>> so get() clones the refcount (O(1))
/// rather than copying the entire JPEG on every frame.
#[derive(Clone)]
pub struct SharedMetadata {
    inner: Arc<RwLock<Metadata>>,
}

impl SharedMetadata {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Metadata::default())),
        }
    }

    pub fn get(&self) -> Metadata {
        self.inner.read().unwrap().clone()
    }

    pub fn set(&self, meta: Metadata) {
        *self.inner.write().unwrap() = meta;
    }
}
