//! Reading the system off the drawing thread.
//!
//! A read of `/proc` on a machine with a few thousand processes takes tens of
//! milliseconds, which is longer than a frame: doing it while drawing would
//! make the window stutter on every refresh. One thread does the reading at
//! the interval the user set and sends each finished sample over a channel,
//! and the window turns the arrival into a repaint.
//!
//! The thread keeps the counters of the previous read itself, so the rates in
//! a sample are the difference between two reads rather than between two
//! frames, and they stay right whatever the window is doing.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::platform::Reader;
use crate::sample::Sample;

/// What the window asks the sampler for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Read once, now, rather than at the end of the interval.
    Refresh,
    /// Stop the thread.
    Stop,
}

/// What the window and the sampler share.
#[derive(Debug)]
struct Shared {
    /// How long to wait between two reads.
    interval: Duration,
    /// What the window asked for, taken by the thread.
    request: Option<Request>,
}

/// The sampler thread, and what it has sent.
pub struct Sampler {
    /// The interval and the request, which the thread waits on.
    shared: Arc<(Mutex<Shared>, Condvar)>,
    /// The samples that came back.
    samples: Receiver<Sample>,
    /// The thread itself, joined when the sampler goes.
    thread: Option<thread::JoinHandle<()>>,
    /// A sample that arrived while the window was busy, which the next frame
    /// takes rather than the one after it.
    pending: Option<Sample>,
    /// How many samples have arrived.
    received: u64,
}

impl Sampler {
    /// Starts the thread, reading every `interval`.
    ///
    /// The thread reads once as soon as it starts, so the window has something
    /// to draw at once, and again after each interval. A read that failed
    /// leaves its error in the sample, which the window shows rather than
    /// hiding. `wake` is called after every sample, which is what asks the
    /// window to draw it: an idle window does not run frames on its own.
    pub fn start(interval: Duration, wake: impl Fn() + Send + 'static) -> Self {
        let (sample_sender, sample_receiver) = mpsc::channel();
        let shared = Arc::new((
            Mutex::new(Shared {
                interval,
                request: None,
            }),
            Condvar::new(),
        ));
        let thread_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("sampler".to_string())
            .spawn(move || run(&sample_sender, &thread_shared, wake))
            .expect("the sampler thread starts");
        Self {
            shared,
            samples: sample_receiver,
            thread: Some(thread),
            pending: None,
            received: 0,
        }
    }

    /// Changes how often the thread reads.
    pub fn set_interval(&self, interval: Duration) {
        let (lock, wake) = &*self.shared;
        if let Ok(mut shared) = lock.lock() {
            shared.interval = interval;
        }
        // The wait in progress was measured against the old interval, so it
        // is ended rather than waited out.
        wake.notify_all();
    }

    /// Asks for a read now, rather than at the end of the interval.
    pub fn refresh(&self) {
        let (lock, wake) = &*self.shared;
        if let Ok(mut shared) = lock.lock() {
            shared.request = Some(Request::Refresh);
        }
        wake.notify_all();
    }

    /// Takes the newest sample, keeping only the latest: a window that was
    /// busy for a while should draw what the machine is doing now, not what
    /// it was doing when the first sample arrived.
    ///
    /// Returns whether a sample arrived, which the caller turns into a
    /// repaint.
    pub fn poll(&mut self) -> bool {
        let mut arrived = false;
        while let Ok(sample) = self.samples.try_recv() {
            self.pending = Some(sample);
            self.received += 1;
            arrived = true;
        }
        arrived
    }

    /// Takes the sample that arrived, if one did.
    pub fn take(&mut self) -> Option<Sample> {
        self.pending.take()
    }

    /// How many samples have come back, which a test waits on.
    pub fn received(&self) -> u64 {
        self.received
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        if let Ok(mut shared) = lock.lock() {
            shared.request = Some(Request::Stop);
        }
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The reading loop.
fn run(samples: &Sender<Sample>, shared: &Arc<(Mutex<Shared>, Condvar)>, wake: impl Fn()) {
    let mut reader = Reader::new();
    loop {
        let started = Instant::now();
        let sample = reader.read();
        if samples.send(sample).is_err() {
            return;
        }
        wake();
        match wait(shared, &started) {
            Next::Again => {}
            Next::Stop => return,
        }
    }
}

/// What the loop does after a read.
enum Next {
    /// Read again.
    Again,
    /// Stop.
    Stop,
}

/// Waits until the next read is due, or until the window asks for one.
///
/// A request ends the wait early, which is what a manual refresh is, and a new
/// interval is measured from the read that just finished rather than from the
/// one before it, so a shortened interval takes effect at once.
fn wait(shared: &Arc<(Mutex<Shared>, Condvar)>, started: &Instant) -> Next {
    let (lock, wake) = &**shared;
    let mut shared = match lock.lock() {
        Ok(shared) => shared,
        Err(_) => return Next::Again,
    };
    loop {
        if let Some(request) = shared.request.take() {
            return match request {
                Request::Refresh => Next::Again,
                Request::Stop => Next::Stop,
            };
        }
        let left = shared.interval.saturating_sub(started.elapsed());
        if left.is_zero() {
            // The kernel coarsens a thread's waking clock the longer it
            // sleeps, so a thread that wakes every second never lets it settle
            // for long; asking for a fine clock keeps the next read on time
            // rather than drifting later every time.
            crate::platform::keep_timers_fine();
            return Next::Again;
        }
        shared = match wake.wait_timeout(shared, left) {
            Ok((shared, _)) => shared,
            Err(_) => return Next::Again,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sampler_produces_samples() {
        let mut sampler = Sampler::start(Duration::from_millis(100), || {});
        let deadline = Instant::now() + Duration::from_secs(20);
        while sampler.received() < 2 && Instant::now() < deadline {
            sampler.poll();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(sampler.received() >= 2, "the thread read twice");
        let sample = sampler.take().expect("a sample is waiting");
        assert!(sample.cpu_count > 0);
        assert!(sample.memory_total > 0);
        assert!(!sample.processes.is_empty());
        assert!(sample.error.is_none());
    }

    #[test]
    fn the_second_sample_has_rates() {
        let mut sampler = Sampler::start(Duration::from_millis(150), || {});
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut last = None;
        while sampler.received() < 3 && Instant::now() < deadline {
            sampler.poll();
            std::thread::sleep(Duration::from_millis(20));
            last = sampler.take().or(last);
        }
        let sample = last.expect("a sample arrived");
        assert!(sample.elapsed > 0.0);
        assert!(
            sample
                .processes
                .iter()
                .any(|process| process.cpu_time > 0.0),
            "a process has used some cpu time"
        );
    }

    #[test]
    fn a_sample_wakes_the_window() {
        let woke = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&woke);
        let _sampler = Sampler::start(Duration::from_millis(50), move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !woke.load(std::sync::atomic::Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            woke.load(std::sync::atomic::Ordering::SeqCst),
            "a sample asks the window to draw it"
        );
    }

    #[test]
    fn a_refresh_arrives_before_the_interval_ends() {
        // A minute between reads, so the only way a second sample can arrive
        // is the refresh being answered.
        let mut sampler = Sampler::start(Duration::from_secs(60), || {});
        let deadline = Instant::now() + Duration::from_secs(20);
        while sampler.received() < 1 && Instant::now() < deadline {
            sampler.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(sampler.poll() || sampler.received() >= 1);
        let before = sampler.received();
        sampler.refresh();
        while sampler.received() <= before && Instant::now() < deadline {
            sampler.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(sampler.received() > before, "the refresh was answered");
    }
}
