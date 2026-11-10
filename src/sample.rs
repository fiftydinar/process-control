//! What one read of the system produced: the processes, and the totals that
//! put them in context.
//!
//! A sample is a value, not a view of live state: the sampler thread computes
//! the rates against the previous sample and sends the result to the window,
//! so drawing never waits on `/proc`.

use std::collections::HashMap;
use std::time::SystemTime;

/// One process, as the table shows it.
#[derive(Clone, Debug, Default)]
pub struct ProcessInfo {
    /// The process id.
    pub pid: i32,
    /// The parent's process id.
    pub ppid: i32,
    /// The name the kernel reports, at most 15 characters.
    pub name: String,
    /// The executable the process runs, when it can be read.
    pub exe: Option<String>,
    /// The Flatpak application id the process runs under, when it is inside
    /// a Flatpak sandbox.
    pub flatpak_id: Option<String>,
    /// The id of the user that owns the process.
    pub uid: u32,
    /// The single letter state: `R`, `S`, `D`, `Z`, `T` and so on.
    pub state: char,
    /// How many threads the process has.
    pub threads: u32,
    /// CPU time used, in seconds.
    pub cpu_time: f64,
    /// CPU usage since the previous sample, as a percentage of one core.
    pub cpu: f32,
    /// Resident set size, in bytes: the memory the process holds.
    pub memory: u64,
    /// Virtual size, in bytes.
    pub virtual_memory: u64,
    /// Bytes read from storage per second.
    pub disk_read: f64,
    /// Bytes written to storage per second.
    pub disk_write: f64,
    /// Bytes received over the network per second.
    pub net_read: f64,
    /// Bytes sent over the network per second.
    pub net_write: f64,
    /// The busiest GPU engine's share of the interval, as a percentage.
    pub gpu: f32,
    /// GPU memory held, in bytes.
    pub gpu_memory: u64,
    /// When the process started.
    pub started: Option<SystemTime>,
}

impl ProcessInfo {
    /// Whether the process is a kernel thread, which has no address space and
    /// no executable of its own.
    pub fn is_kernel_thread(&self) -> bool {
        self.exe.is_none() && self.virtual_memory == 0
    }

    /// The name a row shows: the kernel's name, or the executable's last part
    /// when the kernel reports nothing.
    pub fn display_name(&self) -> &str {
        if !self.name.is_empty() {
            return &self.name;
        }
        self.exe
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .unwrap_or("")
    }

    /// The executable's file name, which is how a desktop entry is matched to
    /// a process on a session that does not say which window it belongs to.
    pub fn executable_name(&self) -> Option<&str> {
        self.exe
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .filter(|name| !name.is_empty())
    }

    /// How long the process has been running, from its start if it has one.
    pub fn uptime(&self) -> Option<u64> {
        self.started
            .and_then(|started| std::time::SystemTime::now().duration_since(started).ok())
            .map(|uptime| uptime.as_secs())
    }
}

/// A window a program has open, as the display server describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Window {
    /// The window's id.
    pub id: u32,
    /// The process that owns it, from `_NET_WM_PID`; 0 when unset.
    pub pid: i32,
    /// The window class, `WM_CLASS`'s second string, such as `firefox`.
    pub class: String,
    /// The window instance, `WM_CLASS`'s first string.
    pub instance: String,
    /// The window title.
    pub title: String,
}

impl Window {
    /// The names a desktop entry's `StartupWMClass` may carry.
    ///
    /// Toolkits disagree about which of `WM_CLASS`'s two strings goes where,
    /// and so do desktop entries, so both are offered rather than guessing.
    /// The class comes first because it is the one the specification names.
    pub fn keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        for value in [&self.class, &self.instance] {
            let value = value.trim();
            if !value.is_empty() && !keys.iter().any(|key| key == value) {
                keys.push(value.to_string());
            }
        }
        keys
    }
}

/// Everything one read of the system produced.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    /// The processes, in the order `/proc` listed them.
    pub processes: Vec<ProcessInfo>,
    /// Total CPU usage, as a percentage of every core together.
    pub cpu: f32,
    /// CPU usage of each core, in the order the kernel numbers them.
    pub cores: Vec<f32>,
    /// Memory in use, in bytes.
    pub memory_used: u64,
    /// Memory the machine has, in bytes.
    pub memory_total: u64,
    /// Swap in use, in bytes.
    pub swap_used: u64,
    /// Swap the machine has, in bytes.
    pub swap_total: u64,
    /// Bytes read from storage per second, over every device.
    pub disk_read: f64,
    /// Bytes written to storage per second, over every device.
    pub disk_write: f64,
    /// Bytes received over the network per second.
    pub net_read: f64,
    /// Bytes sent over the network per second.
    pub net_write: f64,
    /// GPU usage, when the machine reports one.
    pub gpu: Option<f32>,
    /// The busiest GPU's memory in use, in bytes.
    pub gpu_memory: u64,
    /// How many cores the machine has.
    pub cpu_count: usize,
    /// How long the machine has been up, in seconds.
    pub uptime: u64,
    /// The load average over one, five and fifteen minutes.
    pub load: [f32; 3],
    /// How many processes were read.
    pub process_count: usize,
    /// How many of them are runnable.
    pub running: usize,
    /// How long the sample covers, in seconds.
    pub elapsed: f32,
    /// The windows the display server had at this read.
    pub windows: Vec<Window>,
    /// Whether the window list holds only the Xwayland windows, as it does
    /// on a Wayland session, so a program with no window may still be an
    /// application.
    pub windows_are_partial: bool,
    /// Why the sample is empty or partial, when it is.
    pub error: Option<String>,
}

impl Sample {
    /// The share of memory in use, from 0 to 1.
    pub fn memory_fraction(&self) -> f32 {
        crate::format::fraction(self.memory_used, self.memory_total)
    }

    /// The share of swap in use, from 0 to 1.
    pub fn swap_fraction(&self) -> f32 {
        crate::format::fraction(self.swap_used, self.swap_total)
    }

    /// Looks a process up by its id.
    pub fn process(&self, pid: i32) -> Option<&ProcessInfo> {
        self.processes.iter().find(|process| process.pid == pid)
    }

    /// The position of every process in the list, by id.
    pub fn index(&self) -> HashMap<i32, usize> {
        self.processes
            .iter()
            .enumerate()
            .map(|(index, process)| (process.pid, index))
            .collect()
    }
}

/// A process, looked at closely: what the details panel shows.
#[derive(Clone, Debug, Default)]
pub struct ProcessDetails {
    /// The process id.
    pub pid: i32,
    /// The parent's process id.
    pub ppid: i32,
    /// The name the kernel reports.
    pub name: String,
    /// The executable the process runs.
    pub exe: Option<String>,
    /// The command line, arguments included, space separated.
    pub cmdline: String,
    /// The directory the process was started in, when it can be read.
    pub cwd: Option<String>,
    /// The file system root the process sees, which a sandboxed app changes.
    pub root: Option<String>,
    /// The name of the user that owns the process.
    pub user: String,
    /// The id of the user that owns the process.
    pub uid: u32,
    /// The single letter state.
    pub state: char,
    /// How many threads the process has.
    pub threads: u32,
    /// The scheduling priority the process was started at.
    pub nice: i32,
    /// CPU time used, in seconds.
    pub cpu_time: f64,
    /// Resident set size, in bytes.
    pub memory: u64,
    /// Virtual size, in bytes.
    pub virtual_memory: u64,
    /// Swap in use, in bytes.
    pub swap: u64,
    /// Bytes the process has read from storage.
    pub io_read: u64,
    /// Bytes the process has written to storage.
    pub io_write: u64,
    /// How many files and sockets the process has open.
    pub fds: usize,
    /// The out-of-memory killer's score for the process, from 0 to 1000.
    pub oom_score: i32,
    /// When the process started.
    pub started: Option<SystemTime>,
    /// The window class of the application's window, when it has one.
    pub wm_class: Option<String>,
}

/// A signal the details panel can send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// `SIGTERM`: ask the process to stop.
    Term,
    /// `SIGKILL`: stop the process at once, without letting it clean up.
    Kill,
    /// `SIGHUP`: the terminal went away, or the program should reload.
    Hup,
    /// `SIGINT`: the same interrupt Ctrl-C sends.
    Int,
    /// `SIGQUIT`: interrupt with a core dump.
    Quit,
    /// `SIGSTOP`: suspend the process.
    Stop,
    /// `SIGCONT`: resume a suspended process.
    Cont,
    /// `SIGUSR1`: a signal whose meaning the program decides.
    Usr1,
    /// `SIGUSR2`: the second signal whose meaning the program decides.
    Usr2,
}

impl Signal {
    /// Every signal the menu offers, in the order it lists them.
    pub const ALL: [Signal; 9] = [
        Signal::Term,
        Signal::Kill,
        Signal::Hup,
        Signal::Int,
        Signal::Quit,
        Signal::Stop,
        Signal::Cont,
        Signal::Usr1,
        Signal::Usr2,
    ];

    /// The name the menu shows.
    pub fn name(self) -> &'static str {
        match self {
            Signal::Term => "Terminate (SIGTERM)",
            Signal::Kill => "Force stop (SIGKILL)",
            Signal::Hup => "Hang up (SIGHUP)",
            Signal::Int => "Interrupt (SIGINT)",
            Signal::Quit => "Quit (SIGQUIT)",
            Signal::Stop => "Suspend (SIGSTOP)",
            Signal::Cont => "Resume (SIGCONT)",
            Signal::Usr1 => "User signal 1 (SIGUSR1)",
            Signal::Usr2 => "User signal 2 (SIGUSR2)",
        }
    }

    /// The signal's number on Linux, which is what the kernel is given.
    pub fn number(self) -> i32 {
        match self {
            Signal::Term => 15,
            Signal::Kill => 9,
            Signal::Hup => 1,
            Signal::Int => 2,
            Signal::Quit => 3,
            Signal::Stop => 19,
            Signal::Cont => 18,
            Signal::Usr1 => 10,
            Signal::Usr2 => 12,
        }
    }

    /// Whether the signal stops the process for good.
    pub fn stops(self) -> bool {
        matches!(
            self,
            Signal::Term | Signal::Kill | Signal::Quit | Signal::Int
        )
    }

    /// A short verb for the status line, as `sent SIGTERM`.
    pub fn short_name(self) -> &'static str {
        match self {
            Signal::Term => "SIGTERM",
            Signal::Kill => "SIGKILL",
            Signal::Hup => "SIGHUP",
            Signal::Int => "SIGINT",
            Signal::Quit => "SIGQUIT",
            Signal::Stop => "SIGSTOP",
            Signal::Cont => "SIGCONT",
            Signal::Usr1 => "SIGUSR1",
            Signal::Usr2 => "SIGUSR2",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process with only the fields a test cares about set.
    fn process(pid: i32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_string(),
            ..ProcessInfo::default()
        }
    }

    #[test]
    fn a_process_is_found_by_its_id() {
        let sample = Sample {
            processes: vec![process(1, "init"), process(42, "shell")],
            ..Sample::default()
        };
        assert_eq!(sample.process(42).map(|p| p.name.as_str()), Some("shell"));
        assert!(sample.process(7).is_none());
        assert_eq!(sample.index().get(&42), Some(&1));
    }

    #[test]
    fn the_display_name_falls_back_to_the_executable() {
        let mut process = process(1, "");
        process.exe = Some("/usr/lib/firefox/firefox".to_string());
        assert_eq!(process.display_name(), "firefox");
    }

    #[test]
    fn a_kernel_thread_is_one_without_an_address_space() {
        let thread = ProcessInfo {
            pid: 2,
            virtual_memory: 0,
            ..process(2, "kthreadd")
        };
        assert!(thread.is_kernel_thread());
        let mut user = process(3, "bash");
        user.virtual_memory = 4096;
        assert!(!user.is_kernel_thread());
    }

    #[test]
    fn an_empty_sample_has_no_fraction_of_anything() {
        let sample = Sample::default();
        assert_eq!(sample.memory_fraction(), 0.0);
        assert_eq!(sample.swap_fraction(), 0.0);
    }

    #[test]
    fn signals_have_distinct_numbers_and_names() {
        let mut numbers: Vec<i32> = Signal::ALL.iter().map(|signal| signal.number()).collect();
        let count = numbers.len();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers.len(), count);
        for signal in Signal::ALL {
            assert!(!signal.name().is_empty());
            assert!(!signal.short_name().is_empty());
        }
        assert!(Signal::Kill.stops());
        assert!(Signal::Term.stops());
        assert!(!Signal::Stop.stops(), "suspend is not a stop for good");
        assert!(!Signal::Cont.stops());
    }
}
