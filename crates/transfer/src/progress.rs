//! Lock-free progress shared between a transfer thread and the UI.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Connecting = 0,
    /// Sender walks the tree; receiver waits for the manifest.
    Scanning = 1,
    Transferring = 2,
    /// Receiver: renaming the verified item into place.
    Committing = 3,
    Finished = 4,
}

impl Phase {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Scanning,
            2 => Self::Transferring,
            3 => Self::Committing,
            4 => Self::Finished,
            _ => Self::Connecting,
        }
    }
}

#[derive(Debug, Default)]
pub struct Progress {
    phase: AtomicU8,
    total_bytes: AtomicU64,
    done_bytes: AtomicU64,
    total_files: AtomicU64,
    done_files: AtomicU64,
    cancelled: AtomicBool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub phase: Phase,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_files: u64,
    pub done_files: u64,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            phase: Phase::from_u8(self.phase.load(Ordering::Relaxed)),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            done_bytes: self.done_bytes.load(Ordering::Relaxed),
            total_files: self.total_files.load(Ordering::Relaxed),
            done_files: self.done_files.load(Ordering::Relaxed),
        }
    }

    /// Asks the transfer to stop at the next frame boundary. Callers that
    /// need it to stop sooner also shut the socket down.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn set_phase(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Relaxed);
    }

    pub(crate) fn set_totals(&self, bytes: u64, files: u64) {
        self.total_bytes.store(bytes, Ordering::Relaxed);
        self.total_files.store(files, Ordering::Relaxed);
    }

    pub(crate) fn add_bytes(&self, n: u64) {
        self.done_bytes.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn file_done(&self) {
        self.done_files.fetch_add(1, Ordering::Relaxed);
    }
}
