//! Reading the process table from `/proc`.
//!
//! Every process is a directory of small files. They are read in a fixed
//! order, and the process is dropped part way through when it disappears: a
//! process can exit between one file and the next, and a half read one is
//! worse than none.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use crate::error::At;
use crate::sample::ProcessInfo;

/// The counters a process had at the previous read, which the next one turns
/// into rates.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessCounters {
    /// CPU time used, in clock ticks.
    pub cpu_time: u64,
    /// Bytes read from storage.
    pub read_bytes: u64,
    /// Bytes written to storage.
    pub write_bytes: u64,
    /// Bytes received over TCP.
    pub net_received: u64,
    /// Bytes sent over TCP.
    pub net_sent: u64,
    /// GPU time, summed over every engine, in nanoseconds.
    pub gpu_time: u64,
    /// GPU memory held, in bytes.
    pub gpu_memory: u64,
}

/// What one walk over `/proc` produced.
#[derive(Debug, Default)]
pub struct Walk {
    /// The processes, in the order `/proc` listed them.
    pub processes: Vec<ProcessInfo>,
    /// The raw counters of each, for the next walk to measure against.
    pub counters: HashMap<i32, ProcessCounters>,
    /// Why part of the walk failed, when it did.
    pub error: Option<String>,
}

/// Reads every process under `/proc`.
///
/// A process that cannot be read is left out rather than reported: it belongs
/// to another user, or it exited while it was being read, and neither is a
/// failure the window should show.
pub fn walk() -> Walk {
    let mut walk = Walk::default();
    let read_dir = match std::fs::read_dir("/proc").at(Path::new("/proc")) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            walk.error = Some(error);
            return walk;
        }
    };
    let mut entries: Vec<(i32, std::path::PathBuf)> = read_dir
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let pid = name.to_str()?.parse::<i32>().ok()?;
            Some((pid, entry.path()))
        })
        .collect();
    // `/proc` is listed in no particular order; sorting here is what keeps the
    // table's own order stable between two reads.
    entries.sort_unstable_by_key(|(pid, _)| *pid);

    let sockets = crate::platform::net::socket_bytes().unwrap_or_default();
    let boot = boot_time();
    for (pid, directory) in entries {
        let Some(process) = read_process(pid, &directory, &sockets, boot) else {
            continue;
        };
        walk.counters.insert(pid, process.counters);
        walk.processes.push(process.info);
    }
    walk
}

/// One process, read.
struct Read {
    /// The process as the table shows it.
    info: ProcessInfo,
    /// The counters the next read measures against.
    counters: ProcessCounters,
}

/// Reads one process under `directory`.
fn read_process(
    pid: i32,
    directory: &Path,
    sockets: &[super::net::Socket],
    boot: SystemTime,
) -> Option<Read> {
    let stat = std::fs::read_to_string(directory.join("stat")).ok()?;
    let stat = parse_stat(&stat, boot)?;
    let status = std::fs::read_to_string(directory.join("status")).unwrap_or_default();
    let io = parse_io(&std::fs::read_to_string(directory.join("io")).unwrap_or_default());
    let exe = std::fs::read_link(directory.join("exe"))
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok());
    let flatpak_id = flatpak_id(directory);
    let (gpu_time, gpu_memory) = super::gpu::process(directory);
    let (net_received, net_sent) = super::net::socket_bytes_of(directory, sockets);

    let info = ProcessInfo {
        pid,
        ppid: stat.ppid,
        name: stat.name,
        exe,
        flatpak_id,
        uid: parse_uid(&status),
        state: stat.state,
        threads: stat.threads,
        cpu_time: stat.cpu_time,
        memory: stat.rss,
        virtual_memory: stat.vsize,
        started: stat.started,
        gpu_memory,
        ..ProcessInfo::default()
    };
    let counters = ProcessCounters {
        cpu_time: stat.cpu_ticks,
        read_bytes: io.0,
        write_bytes: io.1,
        net_received,
        net_sent,
        gpu_time,
        gpu_memory,
    };
    Some(Read { info, counters })
}

/// The application id of the Flatpak sandbox a process runs in, if any.
///
/// Only a sandboxed process sees `/.flatpak-info`, so that file is a cheap
/// test before reading the environment. `FLATPAK_ID` is what tells the app
/// apart from the `bwrap` and `xdg-dbus-proxy` processes that build its
/// sandbox: those share the file but not the app's environment.
fn flatpak_id(directory: &Path) -> Option<String> {
    if !directory.join("root/.flatpak-info").is_file() {
        return None;
    }
    let environment = std::fs::read(directory.join("environ")).ok()?;
    environment
        .split(|byte| *byte == 0)
        .find_map(|variable| variable.strip_prefix(b"FLATPAK_ID="))
        .and_then(|id| std::str::from_utf8(id).ok())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// How many clock ticks the kernel reports per second.
///
/// It is 100 on every platform this app runs on, but it is asked for rather
/// than assumed, since a kernel can be built with another value.
pub fn clock_ticks() -> f64 {
    static TICKS: OnceLock<f64> = OnceLock::new();
    *TICKS.get_or_init(|| {
        // SAFETY: `sysconf` reads a kernel constant and writes nothing.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if ticks > 0 { ticks as f64 } else { 100.0 }
    })
}

/// The size of a memory page in bytes.
pub fn page_size() -> u64 {
    static SIZE: OnceLock<u64> = OnceLock::new();
    *SIZE.get_or_init(|| {
        // SAFETY: `sysconf` reads a kernel constant and writes nothing.
        let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if size > 0 { size as u64 } else { 4096 }
    })
}

/// The moment the machine booted, from the wall clock less the uptime.
pub fn boot_time() -> SystemTime {
    let uptime = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok())
        .filter(|seconds| *seconds >= 0.0)
        .unwrap_or(0.0);
    SystemTime::now()
        .checked_sub(Duration::from_secs_f64(uptime))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// The fields of `/proc/<pid>/stat` this app reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stat {
    /// The process name, in parentheses.
    pub name: String,
    /// The single letter state.
    pub state: char,
    /// The parent's process id.
    pub ppid: i32,
    /// CPU time used, in clock ticks.
    pub cpu_ticks: u64,
    /// CPU time in user and kernel mode together, in seconds.
    pub cpu_time: f64,
    /// How many threads the process has.
    pub threads: u32,
    /// Virtual size, in bytes.
    pub vsize: u64,
    /// Resident set size, in bytes.
    pub rss: u64,
    /// When the process started.
    pub started: Option<SystemTime>,
}

/// Parses one line of `/proc/<pid>/stat`.
///
/// The name is in parentheses and can hold spaces and parentheses of its own,
/// so it is found from the two ends rather than by splitting the line.
pub fn parse_stat(text: &str, boot: SystemTime) -> Option<Stat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let name = text[open + 1..close].to_string();
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // `rest` starts at field 3, the state, so field n sits at index n - 3.
    let field = |n: usize| rest.get(n - 3).copied().unwrap_or("");
    let state = field(3).chars().next()?;
    let ppid = field(4).parse().ok()?;
    let user = field(14).parse::<u64>().unwrap_or(0);
    let system = field(15).parse::<u64>().unwrap_or(0);
    let threads = field(20).parse().unwrap_or(0);
    let start_ticks = field(22).parse::<u64>().unwrap_or(0);
    let vsize = field(23).parse().unwrap_or(0);
    let rss_pages = field(24).parse::<i64>().unwrap_or(0).max(0) as u64;
    let cpu_ticks = user + system;
    let started = (start_ticks > 0).then(|| {
        boot.checked_add(Duration::from_secs_f64(start_ticks as f64 / clock_ticks()))
            .unwrap_or(boot)
    });
    Some(Stat {
        name,
        state,
        ppid,
        cpu_ticks,
        cpu_time: cpu_ticks as f64 / clock_ticks(),
        threads,
        vsize,
        rss: rss_pages * page_size(),
        started,
    })
}

/// The id of the user that owns a process, from `/proc/<pid>/status`.
fn parse_uid(status: &str) -> u32 {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|values| values.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}

/// The bytes a process read and wrote, from `/proc/<pid>/io`.
fn parse_io(text: &str) -> (u64, u64) {
    let mut read = 0;
    let mut written = 0;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("read_bytes:") {
            read = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("write_bytes:") {
            written = value.trim().parse().unwrap_or(0);
        }
    }
    (read, written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_walk_of_the_process_table_is_fast_enough_to_hide() {
        // The walk runs on the sampler thread, so the window never waits for
        // it; but a walk that took hundreds of milliseconds would still make
        // the numbers late, and a second refresh impossible.
        let _ = walk();
        let started = std::time::Instant::now();
        let mut count = 0;
        for _ in 0..5 {
            count = walk().processes.len();
        }
        let each = started.elapsed() / 5;
        assert!(
            each < Duration::from_millis(250),
            "a walk of {count} processes takes {each:?}"
        );
    }

    /// A boot time a test measures a start against.
    fn boot() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    #[test]
    fn a_stat_line_is_split_from_the_outside_in() {
        // A name can hold spaces and parentheses of its own.
        let line = "1234 (my (odd) proc) S 1 1234 1234 0 -1 4194560 100 0 0 0 \
                    50 25 0 0 20 0 7 0 12345 1000000 250 18446744073709551615 \
                    4096 8192 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let stat = parse_stat(line, boot()).expect("the line parses");
        assert_eq!(stat.name, "my (odd) proc");
        assert_eq!(stat.state, 'S');
        assert_eq!(stat.ppid, 1);
        assert_eq!(stat.threads, 7);
        assert_eq!(stat.cpu_ticks, 75);
        assert!((stat.cpu_time - 75.0 / clock_ticks()).abs() < 1e-9);
        assert_eq!(stat.vsize, 1_000_000);
        assert_eq!(stat.rss, 250 * page_size());
        assert_eq!(
            stat.started,
            Some(boot() + Duration::from_secs_f64(12345.0 / clock_ticks()))
        );
    }

    #[test]
    fn a_line_without_a_name_does_not_parse() {
        assert!(parse_stat("not a stat line", boot()).is_none());
    }

    #[test]
    fn the_uid_comes_from_the_status_line() {
        let status = "Name:\tbash\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\n";
        assert_eq!(parse_uid(status), 1000);
        assert_eq!(parse_uid(""), 0);
    }

    #[test]
    fn io_counters_are_read_by_name() {
        let io = "rchar: 1\nwchar: 2\nsyscr: 3\nsyscw: 4\nread_bytes: 8192\nwrite_bytes: 4096\n";
        assert_eq!(parse_io(io), (8192, 4096));
        assert_eq!(parse_io(""), (0, 0));
    }

    #[test]
    fn the_live_process_table_holds_this_test() {
        // The test binary is a process of its own, and it is in the table.
        let walk = walk();
        assert!(walk.error.is_none());
        let own = std::process::id() as i32;
        let process = walk
            .processes
            .iter()
            .find(|process| process.pid == own)
            .expect("the test process is in the table");
        assert!(process.memory > 0, "a running process holds memory");
        assert!(process.virtual_memory > 0);
        assert!(process.threads >= 1);
        assert!(walk.counters.contains_key(&own));
    }

    #[test]
    fn a_flatpak_sandbox_names_its_application() {
        let directory = std::env::temp_dir().join(format!(
            "process-control-flatpak-info-{}",
            std::process::id()
        ));
        let root = directory.join("root");
        std::fs::create_dir_all(&root).expect("the sandbox root is made");
        std::fs::write(
            root.join(".flatpak-info"),
            "[Application]\nname=org.example.App\n",
        )
        .expect("the flatpak info is written");
        std::fs::write(
            directory.join("environ"),
            b"PATH=/app/bin\0FLATPAK_ID=org.example.App\0",
        )
        .expect("the sandbox environment is written");
        let id = flatpak_id(&directory);
        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
        assert_eq!(id.as_deref(), Some("org.example.App"));
        assert!(flatpak_id(Path::new("/no/such/process")).is_none());
    }

    #[test]
    fn the_clock_tick_rate_is_sane() {
        let ticks = clock_ticks();
        assert!((1.0..=10_000.0).contains(&ticks));
    }

    #[test]
    fn the_boot_time_is_in_the_past() {
        let boot = boot_time();
        assert!(boot < SystemTime::now());
        assert!(
            SystemTime::now().duration_since(boot).unwrap() > Duration::from_secs(1),
            "the machine has been up for a while"
        );
    }
}
