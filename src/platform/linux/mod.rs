//! Reading Linux, from `/proc` and `/sys`.
//!
//! The pieces are split by what they read: the process table, the system-wide
//! counters, the per-process GPU accounting, the sockets, and the windows the
//! display server knows about. [`Reader`] puts them together into one
//! [`Sample`] and keeps the counters the next one measures its rates against.

pub mod gpu;
pub mod net;
pub mod process;
pub mod system;
pub mod windows;

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use crate::sample::{ProcessDetails, Sample, Signal};

/// Reads the system, keeping what the next read measures its rates against.
///
/// One of these belongs to the sampler thread. The first read establishes the
/// counters and so reports no rates; every read afterwards covers the time
/// since the one before it.
#[derive(Debug, Default)]
pub struct Reader {
    /// The system-wide counters of the previous read.
    counters: system::Previous,
    /// The per-process counters of the previous read.
    processes: Option<process::Walk>,
    /// When the previous read started.
    started: Option<Instant>,
}

impl Reader {
    /// A reader that has not read anything yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads the system once.
    pub fn read(&mut self) -> Sample {
        let now = Instant::now();
        let first = self.started.is_none();
        let elapsed = self
            .started
            .map(|started| now.duration_since(started))
            .unwrap_or_default();
        let seconds = elapsed.as_secs_f64().max(0.001);

        let mut sample = Sample::default();
        let (cpu, cores) = system::cpu_times();
        // The first read has no earlier counters to measure against. Its
        // counters are cumulative since boot, and dividing them by a made-up
        // interval would report the machine's whole history as one instant.
        sample.cpu = if first {
            0.0
        } else {
            cpu.percent_since(&self.counters.cpu)
        };
        sample.cores = cores
            .iter()
            .enumerate()
            .map(|(index, times)| {
                if first {
                    0.0
                } else {
                    times.percent_since(
                        self.counters
                            .cores
                            .get(index)
                            .unwrap_or(&Default::default()),
                    )
                }
            })
            .collect();
        sample.cpu_count = cores.len();
        self.counters.cpu = cpu;
        self.counters.cores = cores;

        let memory = system::memory();
        sample.memory_total = memory.total;
        sample.memory_used = memory.used;
        sample.swap_total = memory.swap_total;
        sample.swap_used = memory.swap_used;
        sample.uptime = system::uptime();
        sample.load = system::load();

        let (disk_read, disk_write) = system::disk_io(&system::whole_disks());
        sample.disk_read = if first {
            0.0
        } else {
            disk_read.saturating_sub(self.counters.disk_read) as f64 / seconds
        };
        sample.disk_write = if first {
            0.0
        } else {
            disk_write.saturating_sub(self.counters.disk_write) as f64 / seconds
        };
        self.counters.disk_read = disk_read;
        self.counters.disk_write = disk_write;

        let (net_read, net_write) = system::network_io();
        sample.net_read = if first {
            0.0
        } else {
            system::increase(&net_read, &self.counters.net_read) as f64 / seconds
        };
        sample.net_write = if first {
            0.0
        } else {
            system::increase(&net_write, &self.counters.net_write) as f64 / seconds
        };
        self.counters.net_read = net_read;
        self.counters.net_write = net_write;

        sample.gpu = gpu::busy_percent();
        sample.gpu_memory = gpu::memory_used();

        let mut walk = process::walk();
        rates(&mut walk, self.processes.as_ref(), seconds);
        sample.process_count = walk.processes.len();
        sample.running = walk
            .processes
            .iter()
            .filter(|process| process.state == 'R')
            .count();
        sample.elapsed = if first { 0.0 } else { seconds as f32 };
        sample.error = walk.error.take();
        sample.processes = walk.processes.clone();
        sample.windows = windows::windows();

        self.processes = Some(walk);
        self.started = Some(now);
        sample
    }
}

/// Turns the raw counters of a walk into rates against the previous one.
///
/// A process that has just started has no earlier counters, so its share of
/// the interval is nothing yet: it has not had the time to use any.
fn rates(walk: &mut process::Walk, previous: Option<&process::Walk>, seconds: f64) {
    let ticks = process::clock_ticks();
    for process in &mut walk.processes {
        let Some(now) = walk.counters.get(&process.pid).copied() else {
            continue;
        };
        let Some(before) = previous
            .and_then(|walk| walk.counters.get(&process.pid))
            .copied()
        else {
            process.cpu = 0.0;
            continue;
        };
        let cpu_seconds = now.cpu_time.saturating_sub(before.cpu_time) as f64 / ticks;
        process.cpu = (cpu_seconds / seconds * 100.0) as f32;
        process.disk_read = now.read_bytes.saturating_sub(before.read_bytes) as f64 / seconds;
        process.disk_write = now.write_bytes.saturating_sub(before.write_bytes) as f64 / seconds;
        process.net_read = now.net_received.saturating_sub(before.net_received) as f64 / seconds;
        process.net_write = now.net_sent.saturating_sub(before.net_sent) as f64 / seconds;
        // GPU time is summed over every engine, so a process driving two of
        // them at once can spend more time busy than the interval holds.
        let gpu_seconds = now.gpu_time.saturating_sub(before.gpu_time) as f64 / 1e9;
        process.gpu = (gpu_seconds / seconds * 100.0) as f32;
        process.gpu_memory = now.gpu_memory;
    }
}

/// Reads one process closely, for the details panel.
///
/// A process can exit between any two of these files, so a failure part way
/// through gives what was read rather than nothing: the panel then shows the
/// lines it has.
pub fn details(pid: i32) -> Option<ProcessDetails> {
    let directory = Path::new("/proc").join(pid.to_string());
    let stat = process::parse_stat(
        &std::fs::read_to_string(directory.join("stat")).ok()?,
        process::boot_time(),
    )?;
    let status = std::fs::read_to_string(directory.join("status")).unwrap_or_default();
    let io = std::fs::read_to_string(directory.join("io")).unwrap_or_default();
    let link = |name: &str| {
        std::fs::read_link(directory.join(name))
            .ok()
            .and_then(|path| path.into_os_string().into_string().ok())
    };
    let mut uids = status.lines().filter_map(|line| line.strip_prefix("Uid:"));
    let uid = uids
        .next()
        .and_then(|values| values.split_whitespace().next()?.parse().ok())
        .unwrap_or(0);
    Some(ProcessDetails {
        pid,
        ppid: stat.ppid,
        name: stat.name,
        exe: link("exe"),
        cmdline: cmdline(&std::fs::read(directory.join("cmdline")).unwrap_or_default()),
        cwd: link("cwd"),
        root: link("root"),
        user: user_name(uid),
        uid,
        state: stat.state,
        threads: stat.threads,
        nice: status_number(&status, "Nice:"),
        cpu_time: stat.cpu_time,
        memory: stat.rss,
        virtual_memory: stat.vsize,
        swap: status_value(&status, "VmSwap:"),
        io_read: io_value(&io, "read_bytes:"),
        io_write: io_value(&io, "write_bytes:"),
        fds: std::fs::read_dir(directory.join("fd"))
            .map(|entries| entries.count())
            .unwrap_or(0),
        oom_score: std::fs::read_to_string(directory.join("oom_score"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0),
        started: stat.started,
        wm_class: None,
    })
}

/// Sends a signal to one process.
///
/// A process the user does not own answers with a permission error, which is
/// shown as it is: the app does not ask for more rights than the user has.
pub fn signal(pid: i32, signal: Signal) -> Result<(), String> {
    if pid <= 0 {
        return Err("there is no such process".to_string());
    }
    // SAFETY: `kill` takes a pid and a signal number, and has no other
    // preconditions.
    let result = unsafe { libc::kill(pid, signal.number() as libc::c_int) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "{}: {}",
            signal.short_name(),
            std::io::Error::last_os_error()
        ))
    }
}

/// Keeps the waking clock as fine as the sampler wants it.
///
/// The kernel coarsens a thread's timer as it sleeps for longer, which makes
/// a refresh offer arrive late by a growing margin. The slack is per thread,
/// so setting it here costs only the sampler's own wakeups.
pub fn keep_timers_fine() {
    /// 50 microseconds: the kernel's own starting point, kept instead of
    /// grown.
    const SLACK: libc::c_int = 50_000;
    // SAFETY: `prctl` with `PR_SET_TIMERSLACK` reads one integer.
    unsafe {
        libc::prctl(libc::PR_SET_TIMERSLACK, SLACK, 0, 0, 0);
    }
}

/// The name of the user with this id, or the id as text when there is none.
///
/// The user table is read once: the user column asks for it for every row it
/// draws, and reopening `/etc/passwd` per row per frame would cost more than
/// the drawing around it.
pub fn user_name(uid: u32) -> String {
    static USERS: OnceLock<HashMap<u32, String>> = OnceLock::new();
    let users = USERS.get_or_init(|| {
        std::fs::read_to_string("/etc/passwd")
            .map(|text| {
                text.lines()
                    .filter_map(|line| {
                        let mut fields = line.split(':');
                        let name = fields.next()?;
                        let id: u32 = fields.next()?.parse().ok()?;
                        Some((id, name.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    });
    users.get(&uid).cloned().unwrap_or_else(|| uid.to_string())
}

/// A signed number from a `/proc/<pid>/status` line, such as the nice value.
fn status_number(status: &str, key: &str) -> i32 {
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

/// The bytes a `/proc/<pid>/status` line holds, in bytes.
fn status_value(status: &str, key: &str) -> u64 {
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|number| number.parse::<u64>().ok())
        .map(|kilobytes| kilobytes * 1024)
        .unwrap_or(0)
}

/// The bytes a `/proc/<pid>/io` line holds.
fn io_value(io: &str, key: &str) -> u64 {
    io.lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

/// A command line, whose arguments are NUL separated in the file.
fn cmdline(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .split('\0')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The GPU the machine has, for the About line: the driver's own name.
pub fn gpu_name() -> Option<String> {
    for entry in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let driver = entry.path().join("device/driver");
        if let Ok(target) = std::fs::read_link(driver)
            && let Some(name) = target.file_name()
        {
            return Some(name.to_string_lossy().to_string());
        }
    }
    None
}

/// A duration as the details panel shows it, which is the same as elsewhere.
pub fn uptime_of(details: &ProcessDetails) -> Option<Duration> {
    let started = details.started?;
    SystemTime::now().duration_since(started).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_is_joined_by_spaces() {
        assert_eq!(
            cmdline(b"/usr/bin/thing\0--flag\0value\0"),
            "/usr/bin/thing --flag value"
        );
        assert_eq!(cmdline(b""), "");
        assert_eq!(cmdline(b"/usr/bin/thing\0"), "/usr/bin/thing");
    }

    #[test]
    fn a_status_value_is_read_in_kilobytes() {
        let status = "Name:\tbash\nVmSwap:\t  128 kB\nNice:\t  -5\n";
        assert_eq!(status_value(status, "VmSwap:"), 128 * 1024);
        assert_eq!(status_value(status, "NoSuchKey:"), 0);
    }

    #[test]
    fn an_io_value_is_read_raw() {
        assert_eq!(io_value("read_bytes: 4096\n", "read_bytes:"), 4096);
    }

    #[test]
    fn the_users_own_name_is_found_or_the_id_is_shown() {
        let name = user_name(0);
        assert!(!name.is_empty());
        assert_eq!(user_name(u32::MAX), u32::MAX.to_string());
    }

    #[test]
    fn the_first_read_reports_no_rates() {
        let mut reader = Reader::new();
        let first = reader.read();
        assert_eq!(first.cpu, 0.0, "there is no earlier counter to measure");
        assert_eq!(first.disk_read, 0.0, "boot's disk counters are not a rate");
        assert_eq!(first.disk_write, 0.0, "boot's disk counters are not a rate");
        assert_eq!(first.net_read, 0.0, "boot's net counters are not a rate");
        assert_eq!(first.net_write, 0.0, "boot's net counters are not a rate");
        assert_eq!(first.elapsed, 0.0, "the first read covers no interval");
        assert!(
            first.cores.iter().all(|core| *core == 0.0),
            "no core has an interval to measure"
        );
    }

    #[test]
    fn a_reader_produces_samples_with_rates() {
        let mut reader = Reader::new();
        let first = reader.read();
        assert!(first.memory_total > 0);
        assert!(first.cpu >= 0.0);
        std::thread::sleep(Duration::from_millis(120));
        let second = reader.read();
        assert!(second.elapsed > 0.0);
        assert!(second.cpu >= 0.0 && second.cpu <= 100.0);
        assert!(second.processes.iter().any(|process| process.cpu >= 0.0));
    }

    #[test]
    fn a_read_costs_less_than_a_frame() {
        // The sampler thread pays this, not the drawing thread, but a read
        // that took hundreds of milliseconds would still make the numbers
        // late. A machine with a few hundred processes reads in single
        // milliseconds.
        let mut reader = Reader::new();
        let _ = reader.read();
        let started = Instant::now();
        for _ in 0..5 {
            let _ = reader.read();
        }
        let each = started.elapsed() / 5;
        println!(
            "one read takes {each:?} over {} processes",
            reader
                .processes
                .as_ref()
                .map_or(0, |walk| walk.processes.len())
        );
        assert!(
            each < Duration::from_millis(250),
            "a read takes {each:?}, which would make the window stutter"
        );
    }

    #[test]
    fn details_of_this_test_are_readable() {
        let own = std::process::id() as i32;
        let details = details(own).expect("the test process has details");
        assert_eq!(details.pid, own);
        assert!(!details.name.is_empty());
        assert!(details.memory > 0);
        assert!(details.threads >= 1);
        assert!(details.started.is_some());
        assert!(!details.cmdline.is_empty());
    }

    #[test]
    fn details_of_a_process_that_does_not_exist_are_none() {
        // Above any pid the kernel hands out.
        assert!(details(i32::MAX - 1).is_none());
    }

    #[test]
    fn a_signal_reaches_this_test_process() {
        // SIGCONT on a process that is not stopped does nothing, which is a
        // signal sent and accepted.
        let own = std::process::id() as i32;
        assert!(signal(own, Signal::Cont).is_ok());
        assert!(signal(0, Signal::Cont).is_err());
    }
}
