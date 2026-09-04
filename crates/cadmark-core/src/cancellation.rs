// A cancellation flag shared between the thread that decides to stop and
// the work that must notice: an AI turn and the kernel worker's supervision
// loop both poll it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cloneable handle to one cancellation decision. Cheap to clone and pass
/// into worker threads; cancelling any clone cancels them all.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask everything holding a clone to stop at its next check.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::CancelFlag;

    #[test]
    fn cancelling_one_clone_is_seen_by_every_clone() {
        let flag = CancelFlag::new();
        let observer = flag.clone();
        assert!(!observer.is_cancelled());
        flag.cancel();
        assert!(observer.is_cancelled());
    }
}
