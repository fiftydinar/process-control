//! The system-wide counters: CPU, memory, storage and network.
//!
//! Each of these is a cumulative counter in `/proc`, so a rate is a difference
//! between two reads. The `Previous` type keeps the earlier read, which lets
//! the sampler ask as often as it likes without the answers depending on the
//! interval.

use std::collections::HashMap;

/// One read of a CPU line in `/proc/stat`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    /// Total time the CPU has been up, in clock ticks.
    pub total: u64,
    /// Time spent idle or waiting on I/O, in clock ticks.
    pub idle: u64,
}

impl CpuTimes {
    /// The busy share between `earlier` and this read, from 0 to 100.
    ///
    /// The counter's own total is the interval, so a sample taken early or
    /// late still reports what the CPU did in the time it covered. A counter
    /// that went backwards, which a core hotplugged away and back can make it
    /// do, reports nothing rather than a negative.
    pub fn percent_since(&self, earlier: &CpuTimes) -> f32 {
        let total = self.total.saturating_sub(earlier.total);
        let busy = self.busy().saturating_sub(earlier.busy());
        crate::format::fraction(busy, total) * 100.0
    }

    /// Time spent doing work, in clock ticks.
    fn busy(&self) -> u64 {
        self.total.saturating_sub(self.idle)
    }
}

/// What the machine reported the previous time, which the next read measures
/// its rates against.
#[derive(Debug, Default)]
pub struct Previous {
    /// Every core together.
    pub cpu: CpuTimes,
    /// Each core on its own, in the kernel's order.
    pub cores: Vec<CpuTimes>,
    /// Bytes read from every whole disk.
    pub disk_read: u64,
    /// Bytes written to every whole disk.
    pub disk_write: u64,
    /// Bytes each interface received, by name.
    pub net_read: HashMap<String, u64>,
    /// Bytes each interface sent, by name.
    pub net_write: HashMap<String, u64>,
}

/// The CPU counters, the total and each core.
pub fn cpu_times() -> (CpuTimes, Vec<CpuTimes>) {
    let mut total = CpuTimes::default();
    let mut cores = Vec::new();
    let Ok(text) = std::fs::read_to_string("/proc/stat") else {
        return (total, cores);
    };
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("cpu") else {
            break;
        };
        let (name, values) = rest.split_once(' ').unwrap_or((rest, ""));
        let fields: Vec<u64> = values
            .split_whitespace()
            .filter_map(|field| field.parse().ok())
            .collect();
        // user, nice, system, idle, iowait, irq, softirq, steal. Idle and
        // iowait are the time the CPU spent not working.
        if fields.len() < 5 {
            continue;
        }
        let times = CpuTimes {
            total: fields.iter().take(8).sum(),
            idle: fields[3] + fields[4],
        };
        if name.is_empty() {
            total = times;
        } else {
            cores.push(times);
        }
    }
    (total, cores)
}

/// What `/proc/meminfo` says about memory and swap, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// Memory the machine has.
    pub total: u64,
    /// Memory in use, which is the total less what is available.
    pub used: u64,
    /// Swap the machine has.
    pub swap_total: u64,
    /// Swap in use.
    pub swap_used: u64,
}

/// Reads `/proc/meminfo`.
pub fn memory() -> Memory {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return Memory::default();
    };
    let mut memory = Memory::default();
    let mut available = 0;
    let mut swap_free = 0;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let Some(kilobytes) = value
            .split_whitespace()
            .next()
            .and_then(|number| number.parse::<u64>().ok())
        else {
            continue;
        };
        let bytes = kilobytes * 1024;
        match key {
            "MemTotal" => memory.total = bytes,
            "MemAvailable" => available = bytes,
            "SwapTotal" => memory.swap_total = bytes,
            "SwapFree" => swap_free = bytes,
            _ => {}
        }
    }
    memory.used = memory.total.saturating_sub(available);
    memory.swap_used = memory.swap_total.saturating_sub(swap_free);
    memory
}

/// How long the machine has been up, in seconds.
pub fn uptime() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|text| {
            text.split_whitespace()
                .next()
                .and_then(|seconds| seconds.parse::<f64>().ok())
        })
        .filter(|seconds| *seconds >= 0.0)
        .map_or(0, |seconds| seconds as u64)
}

/// The load average over one, five and fifteen minutes.
pub fn load() -> [f32; 3] {
    let Ok(text) = std::fs::read_to_string("/proc/loadavg") else {
        return [0.0; 3];
    };
    let mut values = text.split_whitespace().map(|field| field.parse().ok());
    let mut load = [0.0; 3];
    for slot in &mut load {
        *slot = values.next().flatten().unwrap_or(0.0);
    }
    load
}

/// The names of the whole disks, whose counters add up to the storage the
/// machine did.
///
/// A partition's counters are already part of its disk's, so adding both would
/// count every read twice. `/sys/block` names the whole disks; a container may
/// not mount it, which is why the names in `/proc/diskstats` are read as well
/// and a device that looks like another one's partition is left out.
pub fn whole_disks() -> Vec<String> {
    if let Ok(entries) = std::fs::read_dir("/sys/block") {
        return entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !is_virtual(name))
            .collect();
    }
    let Ok(text) = std::fs::read_to_string("/proc/diskstats") else {
        return Vec::new();
    };
    let names = device_names(&text);
    names
        .iter()
        .filter(|name| !is_virtual(name))
        .filter(|name| !names.iter().any(|other| is_partition_of(name, other)))
        .cloned()
        .collect()
}

/// Whether a device is one whose counters are not storage the user waited on.
///
/// A loop device holds a file that is already on a disk, and a ram disk is
/// memory: counting either would report the same bytes twice. A device-mapper
/// or software RAID device counts the same bytes as the disks underneath it,
/// so only the underlying disks are counted when they are present.
fn is_virtual(name: &str) -> bool {
    ["loop", "ram", "zram", "fd", "dm-", "md", "bcache"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Whether `name` looks like a partition of `parent`.
///
/// A partition is its disk's name and then a number, `sda1` for `sda`, or an
/// NVMe disk's name and `p` and a number, `nvme0n1p1` for `nvme0n1`.
fn is_partition_of(name: &str, parent: &str) -> bool {
    if name == parent {
        return false;
    }
    let Some(rest) = name.strip_prefix(parent) else {
        return false;
    };
    let digits = rest.strip_prefix('p').unwrap_or(rest);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// The device names in `/proc/diskstats`, in the order they are listed.
fn device_names(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split_whitespace().nth(2).map(str::to_string))
        .collect()
}

/// The bytes every whole disk read and wrote, from `/proc/diskstats`.
pub fn disk_io(disks: &[String]) -> (u64, u64) {
    match std::fs::read_to_string("/proc/diskstats") {
        Ok(text) => parse_diskstats(&text, disks),
        Err(_) => (0, 0),
    }
}

/// The read and written bytes of `disks` in one `/proc/diskstats`.
///
/// The fields are major, minor and name, then the completed reads and the
/// sectors read, and further on the completed writes and the sectors written.
/// A sector is 512 bytes whatever the disk's own sector size is, which is what
/// the kernel reports.
pub fn parse_diskstats(text: &str, disks: &[String]) -> (u64, u64) {
    let mut read = 0;
    let mut written = 0;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 {
            continue;
        }
        if !disks.iter().any(|disk| disk == fields[2]) {
            continue;
        }
        read += fields[5].parse::<u64>().unwrap_or(0) * 512;
        written += fields[9].parse::<u64>().unwrap_or(0) * 512;
    }
    (read, written)
}

/// The bytes every interface but the loopback received and sent, by name.
///
/// Loopback traffic never leaves the machine, so counting it would make a
/// local transfer read as network use.
pub fn network_io() -> (HashMap<String, u64>, HashMap<String, u64>) {
    let Ok(text) = std::fs::read_to_string("/proc/net/dev") else {
        return (HashMap::new(), HashMap::new());
    };
    parse_net_dev(&text)
}

/// The received and sent bytes of every interface in one `/proc/net/dev`.
fn parse_net_dev(text: &str) -> (HashMap<String, u64>, HashMap<String, u64>) {
    let mut read = HashMap::new();
    let mut written = HashMap::new();
    for line in text.lines().skip(2) {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name == "lo" {
            continue;
        }
        let fields: Vec<u64> = values
            .split_whitespace()
            .filter_map(|field| field.parse().ok())
            .collect();
        if fields.len() < 9 {
            continue;
        }
        read.insert(name.to_string(), fields[0]);
        written.insert(name.to_string(), fields[8]);
    }
    (read, written)
}

/// The increase in `now` over `earlier`, summed over every name either has.
pub fn increase(now: &HashMap<String, u64>, earlier: &HashMap<String, u64>) -> u64 {
    now.iter()
        .map(|(name, value)| value.saturating_sub(earlier.get(name).copied().unwrap_or(0)))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_counter_is_the_total_less_the_idle_part() {
        let earlier = CpuTimes {
            total: 1_000,
            idle: 800,
        };
        let now = CpuTimes {
            total: 1_200,
            idle: 900,
        };
        // 200 ticks passed, 100 of them busy.
        assert_eq!(now.percent_since(&earlier), 50.0);
    }

    #[test]
    fn a_counter_that_did_not_move_reports_nothing() {
        let times = CpuTimes {
            total: 100,
            idle: 40,
        };
        assert_eq!(times.percent_since(&times), 0.0);
    }

    #[test]
    fn counters_that_went_backwards_report_nothing() {
        let earlier = CpuTimes {
            total: 1_000,
            idle: 500,
        };
        let now = CpuTimes {
            total: 100,
            idle: 90,
        };
        assert_eq!(now.percent_since(&earlier), 0.0);
    }

    #[test]
    fn the_increase_is_per_interface() {
        let earlier = HashMap::from([("eth0".to_string(), 100), ("wlan0".to_string(), 50)]);
        let now = HashMap::from([
            ("eth0".to_string(), 300),
            ("wlan0".to_string(), 50),
            ("tun0".to_string(), 10),
        ]);
        assert_eq!(increase(&now, &earlier), 210);
    }

    #[test]
    fn a_disk_counters_are_added_up_in_bytes() {
        let text = "\
 259       0 nvme0n1 100 0 2048 0 50 0 1024 0 0 0 0
 259       1 nvme0n1p1 10 0 512 0 5 0 256 0 0 0 0
   8       0 sda 70 0 4096 0 20 0 2048 0 0 0 0
";
        let disks = vec!["nvme0n1".to_string(), "sda".to_string()];
        let (read, written) = parse_diskstats(text, &disks);
        assert_eq!(
            read,
            (2048 + 4096) * 512,
            "the partition is not added twice"
        );
        assert_eq!(written, (1024 + 2048) * 512);
    }

    #[test]
    fn stacked_devices_are_not_counted_alongside_their_parts() {
        assert!(is_virtual("loop0"));
        assert!(is_virtual("ram0"));
        assert!(is_virtual("dm-1"));
        assert!(is_virtual("md0"));
        assert!(!is_virtual("sda"));
        assert!(!is_virtual("nvme0n1"));
    }

    #[test]
    fn a_partition_is_recognised_however_its_disk_is_named() {
        assert!(is_partition_of("sda1", "sda"));
        assert!(is_partition_of("nvme0n1p2", "nvme0n1"));
        assert!(!is_partition_of("sda", "sda"));
        assert!(!is_partition_of("sdb", "sda"));
        assert!(!is_partition_of("nvme1n1", "nvme0n1"));
    }

    #[test]
    fn network_counters_skip_the_loopback_interface() {
        let text = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0
  eth0: 300 3 0 0 0 0 0 0 400 4 0 0 0 0 0 0
";
        let (read, written) = parse_net_dev(text);
        assert_eq!(read.get("eth0"), Some(&300));
        assert_eq!(written.get("eth0"), Some(&400));
        assert!(!read.contains_key("lo"));
    }

    #[test]
    fn the_machine_reads_a_plausible_sample() {
        let (total, cores) = cpu_times();
        assert!(total.total > 0, "the cpu counters moved at some point");
        assert!(!cores.is_empty(), "the machine has cores");
        assert!(total.idle <= total.total);
        let memory = memory();
        assert!(memory.total > 0);
        assert!(memory.used <= memory.total);
        assert!(memory.swap_used <= memory.swap_total);
        assert!(uptime() > 0);
        assert!(load()[0] >= 0.0);
        // Reading the counters is enough: a container may mount no disk that
        // reports traffic, and this must not be an error.
        let _ = disk_io(&whole_disks());
        let (received, sent) = network_io();
        assert_eq!(
            received.len(),
            sent.len(),
            "an interface is read for both directions"
        );
        assert!(received.keys().all(|name| sent.contains_key(name)));
    }
}
