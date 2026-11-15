//! Each running pane's process, for the list of what Layer's panes started
//! (HEN-6, `commands/processes.rs`).

use std::sync::atomic::Ordering;

use super::{PaneInfo, PtyManager};

impl PtyManager {
    /// Every running pane with its process, the leader of its group.
    pub fn running_pids(&self) -> Vec<(PaneInfo, u32)> {
        self.panes
            .lock()
            .values()
            .filter_map(|p| {
                let info = p.meta.lock().snapshot(p.watched.load(Ordering::Acquire));
                let pid = p.pid?;
                info.running.then_some((info, pid))
            })
            .collect()
    }
}
