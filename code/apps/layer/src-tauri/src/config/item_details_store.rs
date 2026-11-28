use super::{AppConfig, ConfigStore};
use crate::error::Result;

impl ConfigStore {
    /// Local details never queue workspace writes. Like `mirror`, publish
    /// only after persistence, and merge against the snapshot under `disk`.
    /// Callers must use the command blocking pool and must not reenter the store.
    pub(crate) fn edit_item_details<T>(&self, edit: impl FnOnce(&mut AppConfig) -> Result<(T, bool)>) -> Result<T> {
        let _disk = self.disk.lock();
        let mut snapshot = self.inner.read().clone();
        let (out, changed) = edit(&mut snapshot)?;
        if changed {
            self.persist(&snapshot)?;
            *self.inner.write() = snapshot;
        }
        Ok(out)
    }
}
