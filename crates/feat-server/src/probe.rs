//! Reading the machine: small pure parsers that take a file's text, and the readers that feed
//! them. Inside the bot's Docker container `/proc/stat`, `/proc/meminfo`, `/proc/uptime` and
//! `/proc/loadavg` show the whole host, which is what the page wants; `/sys/fs/cgroup` is the
//! container's own. A file that can't be read or parsed is `None`, never a panic or a log line.

use std::path::{Path, PathBuf};
use std::time::Instant;

/// CPU time from the aggregate `cpu` line of `/proc/stat`, in clock ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

/// The `cpu` line of `/proc/stat` and how many `cpuN` lines follow it (the cores).
pub fn parse_stat(text: &str) -> Option<(CpuTimes, u32)> {
    let mut lines = text.lines();
    let first = lines.next()?;
    let mut f = first.split_ascii_whitespace();
    if f.next()? != "cpu" {
        return None;
    }
    // user nice system idle iowait irq softirq steal; guest time is already inside user.
    let v: Vec<u64> = f.take(8).map(|x| x.parse().ok()).collect::<Option<_>>()?;
    if v.len() < 4 {
        return None;
    }
    let total: u64 = v.iter().sum();
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    let cores = lines.take_while(|l| l.starts_with("cpu")).filter(|l| l.as_bytes().get(3).is_some_and(u8::is_ascii_digit)).count() as u32;
    Some((CpuTimes { busy: total - idle, total }, cores.max(1)))
}

/// Busy share between two readings, in hundredths of a percent (0 to 10000).
pub fn cpu_share(a: CpuTimes, b: CpuTimes) -> Option<i64> {
    let total = b.total.checked_sub(a.total)?;
    let busy = b.busy.checked_sub(a.busy)?;
    if total == 0 {
        return None;
    }
    Some(((busy * 10_000 + total / 2) / total).min(10_000) as i64)
}

/// `MemTotal` and `MemAvailable` from `/proc/meminfo`, in KiB: `(total, used)`, used being
/// total minus available (what `free` calls used, give or take the cache it can drop).
pub fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let (mut total, mut avail) = (None, None);
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("MemTotal:") {
            total = kib(v);
        } else if let Some(v) = line.strip_prefix("MemAvailable:") {
            avail = kib(v);
        }
        if total.is_some() && avail.is_some() {
            break;
        }
    }
    let (total, avail) = (total?, avail?);
    Some((total, total.saturating_sub(avail)))
}

/// "  16314560 kB" as 16314560.
fn kib(v: &str) -> Option<u64> {
    v.split_ascii_whitespace().next()?.parse().ok()
}

/// Seconds since boot, from `/proc/uptime`.
pub fn parse_uptime(text: &str) -> Option<i64> {
    let s: f64 = text.split_ascii_whitespace().next()?.parse().ok()?;
    (s.is_finite() && s >= 0.0).then_some(s as i64)
}

/// The one-minute load average from `/proc/loadavg`, in hundredths.
pub fn parse_loadavg(text: &str) -> Option<i64> {
    let l: f64 = text.split_ascii_whitespace().next()?.parse().ok()?;
    (l.is_finite() && l >= 0.0).then_some((l * 100.0).round() as i64)
}

/// `usage_usec` from a cgroup's `cpu.stat`.
pub fn parse_cpu_stat(text: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix("usage_usec ")).and_then(|v| v.trim().parse().ok())
}

/// One key of a cgroup's `memory.stat` (`inactive_file`), in bytes.
pub fn parse_memory_stat(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(' ')).and_then(|v| v.trim().parse().ok())
}

/// `VmRSS` from `/proc/self/status`, in KiB.
pub fn parse_vmrss(text: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix("VmRSS:")).and_then(kib)
}

/// utime + stime from `/proc/self/stat`, in clock ticks. The name in brackets may hold spaces,
/// so the fields are counted from the last `)`.
pub fn parse_self_stat(text: &str) -> Option<u64> {
    let rest = &text[text.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_ascii_whitespace().collect();
    // After the name: state is field 3, utime 14 and stime 15.
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    Some(utime + stime)
}

/// A hwmon or thermal zone reading, millidegrees, in tenths of a degree.
pub fn parse_millideg(text: &str) -> Option<i64> {
    let m: i64 = text.trim().parse().ok()?;
    // Below freezing or above 150 °C is a sensor that isn't wired to anything.
    (0..150_000).contains(&m).then_some((m + 50) / 100)
}

/// The CPU's name from `/proc/cpuinfo`, without the trademarks and clock:
/// "Intel(R) Core(TM) i5-8500T CPU @ 2.10GHz" is "Intel Core i5-8500T".
pub fn parse_cpu_model(text: &str) -> Option<String> {
    let raw = text.lines().find_map(|l| l.strip_prefix("model name"))?.split_once(':')?.1;
    let raw = raw.split(" @ ").next().unwrap_or(raw);
    let mut words: Vec<&str> = Vec::new();
    for w in raw.split_ascii_whitespace() {
        let w = w.trim_end_matches("(R)").trim_end_matches("(TM)").trim_end_matches("(tm)");
        if w.is_empty() || w == "CPU" || w == "Processor" || w.ends_with("-Core") {
            continue;
        }
        words.push(w);
    }
    let name = words.join(" ");
    (!name.is_empty()).then_some(name)
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

// ---- the machine as a whole ----

pub fn cpu_times() -> Option<(CpuTimes, u32)> {
    read("/proc/stat").as_deref().and_then(parse_stat)
}

pub fn memory() -> Option<(u64, u64)> {
    read("/proc/meminfo").as_deref().and_then(parse_meminfo)
}

pub fn uptime() -> Option<i64> {
    read("/proc/uptime").as_deref().and_then(parse_uptime)
}

pub fn load() -> Option<i64> {
    read("/proc/loadavg").as_deref().and_then(parse_loadavg)
}

pub fn cpu_model() -> Option<String> {
    read("/proc/cpuinfo").as_deref().and_then(parse_cpu_model)
}

/// `(used, total)` bytes of the filesystem holding `dir`, as `df` counts them (used is total
/// minus free, reserved blocks included).
pub fn disk(dir: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs is plain old data, so all zeroes is a valid value to be overwritten.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a valid NUL-terminated string and `s` a valid place to write to.
    if unsafe { libc::statvfs(path.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let block = s.f_frsize as u64;
    let total = (s.f_blocks as u64).checked_mul(block)?;
    let free = (s.f_bfree as u64).checked_mul(block)?;
    (total > 0).then(|| (total.saturating_sub(free), total))
}

/// The folder the database lives in (`/data` in the image); the working folder for a bare
/// file name or the preview's in-memory database.
pub fn data_dir(database_path: &str) -> PathBuf {
    match Path::new(database_path).parent() {
        Some(p) if !p.as_os_str().is_empty() && database_path != ":memory:" => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Where a CPU temperature can be read, found once at start: a `coretemp` or `k10temp` hwmon
/// sensor, else the `x86_pkg_temp` thermal zone, else the first zone that reads.
pub fn find_temp_sensor() -> Option<PathBuf> {
    let mut hwmon: Vec<PathBuf> = std::fs::read_dir("/sys/class/hwmon").ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    hwmon.sort();
    for dir in &hwmon {
        let name = read(&dir.join("name").to_string_lossy()).unwrap_or_default();
        if matches!(name.trim(), "coretemp" | "k10temp" | "zenpower" | "cpu_thermal") {
            let p = dir.join("temp1_input");
            if read_temp(&p).is_some() {
                return Some(p);
            }
        }
    }
    let mut zones: Vec<PathBuf> = std::fs::read_dir("/sys/class/thermal")
        .ok()
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("thermal_zone"))).collect())
        .unwrap_or_default();
    zones.sort();
    let pkg = zones.iter().find(|z| read(&z.join("type").to_string_lossy()).is_some_and(|t| t.trim() == "x86_pkg_temp"));
    pkg.into_iter().chain(zones.iter()).map(|z| z.join("temp")).find(|p| read_temp(p).is_some())
}

pub fn read_temp(path: &Path) -> Option<i64> {
    std::fs::read_to_string(path).ok().as_deref().and_then(parse_millideg)
}

// ---- the bot ----

/// What the bot's own numbers cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The container's cgroup: the bot, Caddy and Relay together.
    Container,
    /// Only the bot's process (running outside a container).
    Process,
}

/// The container's cgroup when the bot runs in one: the root cgroup a process outside a
/// container sees has no `memory.current`, so its `cpu.stat` would be the whole machine's.
pub fn bot_scope() -> Scope {
    if Path::new("/sys/fs/cgroup/memory.current").exists() && Path::new("/sys/fs/cgroup/cpu.stat").exists() { Scope::Container } else { Scope::Process }
}

/// The bot's memory in KiB: the cgroup's minus the file cache it can drop (as `docker stats`
/// counts it), or the process's resident size.
pub fn bot_memory(scope: Scope) -> Option<u64> {
    match scope {
        Scope::Container => {
            let current: u64 = read("/sys/fs/cgroup/memory.current")?.trim().parse().ok()?;
            let inactive = read("/sys/fs/cgroup/memory.stat").as_deref().and_then(|t| parse_memory_stat(t, "inactive_file")).unwrap_or(0);
            Some(current.saturating_sub(inactive) / 1024)
        }
        Scope::Process => read("/proc/self/status").as_deref().and_then(parse_vmrss),
    }
}

/// CPU time the bot has used, in microseconds, and when it was read.
pub fn bot_cpu(scope: Scope) -> Option<(u64, Instant)> {
    let used = match scope {
        Scope::Container => read("/sys/fs/cgroup/cpu.stat").as_deref().and_then(parse_cpu_stat)?,
        // Linux reports /proc times in USER_HZ, which is 100 on every architecture it ships.
        Scope::Process => read("/proc/self/stat").as_deref().and_then(parse_self_stat)? * 10_000,
    };
    Some((used, Instant::now()))
}

/// The bot's share of the whole machine between two readings, in hundredths of a percent.
pub fn bot_share(a: (u64, Instant), b: (u64, Instant), cores: u32) -> Option<i64> {
    let used = b.0.checked_sub(a.0)?;
    let wall = b.1.checked_duration_since(a.1)?.as_micros() as u64 * cores.max(1) as u64;
    (wall > 0).then(|| ((used * 10_000 + wall / 2) / wall).min(10_000) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  68094933 5164328 9199651 1319368301 1539180 3730445 560859 0 0 0
cpu0 4632140 282128 950046 81692801 252814 177795 83019 0 0 0
cpu1 4601070 308181 820928 81877059 215835 152944 58820 0 0 0
intr 123 4 5
ctxt 99
";

    #[test]
    fn proc_stat() {
        let (t, cores) = parse_stat(STAT).unwrap();
        assert_eq!(cores, 2);
        assert_eq!(t.total, 68094933 + 5164328 + 9199651 + 1319368301 + 1539180 + 3730445 + 560859);
        assert_eq!(t.busy, t.total - 1319368301 - 1539180);
        assert!(parse_stat("").is_none());
        assert!(parse_stat("cpu0 1 2 3 4").is_none());
        assert!(parse_stat("cpu x y z").is_none());
    }

    #[test]
    fn busy_share_between_readings() {
        let a = CpuTimes { busy: 100, total: 1000 };
        assert_eq!(cpu_share(a, CpuTimes { busy: 150, total: 1200 }), Some(2500));
        assert_eq!(cpu_share(a, a), None);
        // A counter that went backwards (a reboot between readings) is no reading.
        assert_eq!(cpu_share(a, CpuTimes { busy: 50, total: 2000 }), None);
    }

    #[test]
    fn meminfo() {
        let t = "MemTotal:       16314560 kB\nMemFree:         1000000 kB\nMemAvailable:   12000000 kB\nBuffers: 1 kB\n";
        assert_eq!(parse_meminfo(t), Some((16314560, 4314560)));
        assert_eq!(parse_meminfo("MemTotal: 5 kB\n"), None);
        assert_eq!(parse_meminfo("garbage"), None);
    }

    #[test]
    fn uptime_and_load() {
        assert_eq!(parse_uptime("874521.39 13193683.01\n"), Some(874521));
        assert_eq!(parse_uptime(""), None);
        assert_eq!(parse_loadavg("0.68 1.69 1.26 1/2135 2433207\n"), Some(68));
        assert_eq!(parse_loadavg("nan 1 1"), None);
    }

    #[test]
    fn cgroup_files() {
        assert_eq!(parse_cpu_stat("usage_usec 867502179057\nuser_usec 7\nsystem_usec 1\n"), Some(867502179057));
        assert_eq!(parse_cpu_stat("user_usec 7\n"), None);
        let ms = "anon 1000\nfile 5000\ninactive_file 4096\nactive_file 904\n";
        assert_eq!(parse_memory_stat(ms, "inactive_file"), Some(4096));
        assert_eq!(parse_memory_stat(ms, "file"), Some(5000));
        assert_eq!(parse_memory_stat(ms, "shmem"), None);
    }

    #[test]
    fn own_process() {
        assert_eq!(parse_vmrss("Name:\tbot\nVmPeak:\t 99 kB\nVmRSS:\t   20480 kB\n"), Some(20480));
        // A name with spaces and brackets doesn't shift the fields.
        let stat = "4242 (bot (main) x) S 1 4242 4242 0 -1 4194560 3051 0 0 0 125 37 0 0 20 0 3 0 100 0 0";
        assert_eq!(parse_self_stat(stat), Some(162));
        assert_eq!(parse_self_stat("4242 (bot) S 1"), None);
    }

    #[test]
    fn temperatures() {
        assert_eq!(parse_millideg("46000\n"), Some(460));
        assert_eq!(parse_millideg("46049"), Some(460));
        assert_eq!(parse_millideg("-1000"), None);
        assert_eq!(parse_millideg("255000"), None);
        assert_eq!(parse_millideg(""), None);
    }

    #[test]
    fn cpu_names() {
        let t = "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Core(TM) i5-8500T CPU @ 2.10GHz\n";
        assert_eq!(parse_cpu_model(t).as_deref(), Some("Intel Core i5-8500T"));
        let amd = "model name\t: AMD Ryzen 9 9950X3D 16-Core Processor\n";
        assert_eq!(parse_cpu_model(amd).as_deref(), Some("AMD Ryzen 9 9950X3D"));
        assert_eq!(parse_cpu_model("nothing here"), None);
    }

    #[test]
    fn bot_share_is_of_the_whole_machine() {
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_secs(60);
        // 6 s of CPU in a minute on 6 cores is 1.67% of the machine.
        assert_eq!(bot_share((0, t0), (6_000_000, t1), 6), Some(167));
        assert_eq!(bot_share((10, t1), (5, t0), 6), None);
    }

    #[test]
    fn data_folder() {
        assert_eq!(data_dir("/data/bot-v2.db"), PathBuf::from("/data"));
        assert_eq!(data_dir("bot.db"), PathBuf::from("."));
        assert_eq!(data_dir(":memory:"), PathBuf::from("."));
    }

    #[test]
    fn readers_never_panic_here() {
        // Whatever machine runs the tests, every reader answers or says it can't.
        let _ = (cpu_times(), memory(), uptime(), load(), cpu_model(), find_temp_sensor());
        let scope = bot_scope();
        let _ = (bot_memory(scope), bot_cpu(scope));
        assert!(disk(Path::new(".")).is_some_and(|(u, t)| u <= t));
        assert!(disk(Path::new("/no/such/folder")).is_none());
    }
}
