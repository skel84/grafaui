//! Shared cancellation and revision checks, independent of GPUI and networking.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> crate::Result<()> {
        if self.is_cancelled() {
            Err("request cancelled".into())
        } else {
            Ok(())
        }
    }
}

/// Replacing or dropping a run cancels its queued work. In-flight blocking HTTP
/// calls finish within the timeout; their results cannot enter a newer run.
#[derive(Debug)]
pub struct Run {
    revision: u64,
    cancellation: Cancellation,
}

impl Run {
    pub fn new(revision: u64) -> Self {
        Self {
            revision,
            cancellation: Cancellation::default(),
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }
    pub fn accepts(&self, revision: u64) -> bool {
        self.revision == revision && !self.cancellation.is_cancelled()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_cancels_old_work_and_rejects_out_of_order_results() {
        let mut run = Run::new(1);
        let old_token = run.cancellation();
        assert!(run.accepts(1));
        run = Run::new(2);
        assert!(old_token.is_cancelled());
        assert!(old_token.check().is_err());
        assert!(!run.accepts(1));
        assert!(run.accepts(2));
        let token = run.cancellation();
        drop(run);
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancellation_is_shared_across_workers_and_prevents_acceptance() {
        let run = Run::new(7);
        let worker = run.cancellation();
        let other = worker.clone();
        std::thread::spawn(move || worker.cancel()).join().unwrap();
        assert!(other.is_cancelled());
        assert!(!run.accepts(7));
    }
}
