//! Measures how much the IME processes delay other applications: a
//! normal-priority thread repeatedly sleeps 1 ms and records how late it wakes
//! up. When the converter's threads run at a higher priority and keep the CPUs
//! busy, ordinary application threads (like the one the user types into) wake
//! up late.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub struct WakeupProbe {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<Vec<Duration>>,
}

impl WakeupProbe {
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut lateness = Vec::new();
            while !flag.load(Ordering::Relaxed) {
                let start = Instant::now();
                std::thread::sleep(Duration::from_millis(1));
                lateness.push(start.elapsed().saturating_sub(Duration::from_millis(1)));
            }
            lateness
        });
        Self { stop, handle }
    }

    pub fn finish(self) -> Vec<Duration> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().unwrap_or_default()
    }
}
