use crate::storage::Storage;
use std::sync::Mutex;

pub struct AppState {
    pub storage: Mutex<Storage>,
}

impl AppState {
    pub fn new() -> Self {
        let storage = Storage::with_default_path();
        storage.ensure_dirs().unwrap();
        Self {
            storage: Mutex::new(storage),
        }
    }
}
