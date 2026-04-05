use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Metadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub image_key: String,
    pub cover_art: Option<Vec<u8>>,
}

#[derive(Clone)]
pub struct SharedMetadata {
    inner: Arc<Mutex<Metadata>>,
}

impl SharedMetadata {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Metadata::default())),
        }
    }

    pub fn get(&self) -> Metadata {
        let inner = self.inner.lock().unwrap();
        Metadata {
            title: inner.title.clone(),
            artist: inner.artist.clone(),
            album: inner.album.clone(),
            image_key: inner.image_key.clone(),
            cover_art: None, // use get_cover_art() for the expensive JPEG clone
        }
    }

    pub fn set(&self, meta: Metadata) {
        *self.inner.lock().unwrap() = meta;
    }

    pub fn get_cover_art(&self) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().cover_art.clone()
    }
}
