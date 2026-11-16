use serde::Serialize;
use std::collections::HashMap;
use std::ffi::CStr;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PROC_PIDTBSDINFO: i32 = 3;
const PROC_PIDTASKINFO: i32 = 4;
const RUSAGE_INFO_V4: i32 = 4;
const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ProcTaskInfoRaw {
    pti_virtual_size: u64,
    pti_resident_size: u64,
    pti_total_user: u64,
    pti_total_system: u64,
    pti_threads_user: u64,
    pti_threads_system: u64,
    pti_policy: i32,
    pti_faults: i32,
    pti_pageins: i32,
    pti_cow_faults: i32,
    pti_messages_sent: i32,
    pti_messages_received: i32,
    pti_syscalls_mach: i32,
    pti_syscalls_unix: i32,
    pti_csw: i32,
    pti_threadnum: i32,
    pti_numrunning: i32,
    pti_priority: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ProcBsdInfoRaw {
    pbi_flags: u32,
    pbi_status: u32,
    pbi_xstatus: u32,
    pbi_pid: u32,
    pbi_ppid: u32,
    pbi_uid: u32,
    pbi_gid: u32,
    pbi_ruid: u32,
    pbi_rgid: u32,
    pbi_svuid: u32,
    pbi_svgid: u32,
    rfu_1: u32,
    pbi_comm: [u8; 16],
    pbi_name: [u8; 32],
    pbi_nfiles: u32,
    pbi_pgid: u32,
    pbi_pjobc: u32,
    e_tdev: u32,
    e_tpgid: u32,
    pbi_nice: i32,
    pbi_start_tvsec: u64,
    pbi_start_tvusec: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RusageInfoV4Raw {
    ri_uuid: [u8; 16],
    ri_user_time: u64,
    ri_system_time: u64,
    ri_pkg_idle_wkups: u64,
    ri_interrupt_wkups: u64,
    ri_pageins: u64,
    ri_wired_size: u64,
    ri_resident_size: u64,
    ri_phys_footprint: u64,
    ri_proc_start_abstime: u64,
    ri_proc_exit_abstime: u64,
    ri_child_user_time: u64,
    ri_child_system_time: u64,
    ri_child_pkg_idle_wkups: u64,
    ri_child_interrupt_wkups: u64,
    ri_child_pageins: u64,
    ri_child_elapsed_abstime: u64,
    ri_diskio_bytesread: u64,
    ri_diskio_byteswritten: u64,
    _rest: [u8; 136],
}

unsafe extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    fn proc_listallpids(buffer: *mut libc::c_void, buffersize: i32) -> i32;
    fn proc_pidinfo(
        pid: i32,
        flavor: i32,
        arg: u64,
        buffer: *mut libc::c_void,
        buffersize: i32,
    ) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut libc::c_void, buffersize: u32) -> i32;
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut libc::c_void) -> i32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SortColumn {
    Cpu,
    Memory,
    #[value(alias = "network")]
    NetDown,
    NetUp,
    App,
    Disk,
    Pid,
    Threads,
}

impl SortColumn {
    pub fn next(self) -> Self {
        match self {
            SortColumn::Pid => SortColumn::App,
            SortColumn::App => SortColumn::Cpu,
            SortColumn::Cpu => SortColumn::Memory,
            SortColumn::Memory => SortColumn::NetDown,
            SortColumn::NetDown => SortColumn::NetUp,
            SortColumn::NetUp => SortColumn::Disk,
            SortColumn::Disk => SortColumn::Threads,
            SortColumn::Threads => SortColumn::Pid,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            SortColumn::Pid => SortColumn::Threads,
            SortColumn::App => SortColumn::Pid,
            SortColumn::Cpu => SortColumn::App,
            SortColumn::Memory => SortColumn::Cpu,
            SortColumn::NetDown => SortColumn::Memory,
            SortColumn::NetUp => SortColumn::NetDown,
            SortColumn::Disk => SortColumn::NetUp,
            SortColumn::Threads => SortColumn::Disk,
        }
    }

    pub fn default_descending(self) -> bool {
        match self {
            SortColumn::App | SortColumn::Pid => false,
            _ => true,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortColumn::Cpu => "CPU%",
            SortColumn::Memory => "Memory",
            SortColumn::NetDown => "Net ↓/s",
            SortColumn::NetUp => "Net ↑/s",
            SortColumn::App => "App/Process",
            SortColumn::Disk => "Disk I/O",
            SortColumn::Pid => "PID",
            SortColumn::Threads => "Threads",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessFilter {
    All,
    AppsOnly,
}

impl ProcessFilter {
    pub fn next(self) -> Self {
        match self {
            ProcessFilter::All => ProcessFilter::AppsOnly,
            ProcessFilter::AppsOnly => ProcessFilter::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProcessFilter::All => "All",
            ProcessFilter::AppsOnly => "Apps",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub name: String,
    pub app_name: Option<String>,
    pub is_app: bool,
    pub cpu_pct: f64,
    pub mem_bytes: u64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub net_total_bytes: u64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub threads: i32,
}

impl ProcessInfo {
    pub fn display_name(&self) -> &str {
        if self.is_app {
            self.app_name.as_deref().unwrap_or(&self.name)
        } else {
            &self.name
        }
    }

    pub fn disk_bps(&self) -> f64 {
        self.disk_read_bps + self.disk_write_bps
    }
}

#[derive(Clone, Copy)]
struct PrevCpuDisk {
    cpu_ns: u64,
    disk_read: u64,
    disk_write: u64,
    start_tvsec: u64,
}

#[derive(Clone)]
struct CachedMeta {
    start_tvsec: u64,
    name: String,
    app_name: Option<String>,
    is_app: bool,
}

#[derive(Clone, Default)]
struct NetSnapshot {
    entries: HashMap<u32, (String, u64, u64)>,
}

struct NetPrev {
    bytes_in: u64,
    bytes_out: u64,
    at: Instant,
}

pub struct ProcessMonitor {
    timebase_numer: u64,
    timebase_denom: u64,
    prev_cpu: HashMap<u32, PrevCpuDisk>,
    prev_sample_time: Instant,
    meta_cache: HashMap<u32, CachedMeta>,
    net_latest: Arc<Mutex<NetSnapshot>>,
    net_prev: HashMap<u32, NetPrev>,
    net_rates: HashMap<u32, (f64, f64, u64, u64)>,
    stop_flag: Arc<AtomicBool>,
}

impl ProcessMonitor {
    pub fn new() -> Self {
        let mut tb = MachTimebaseInfo { numer: 1, denom: 1 };
        unsafe {
            mach_timebase_info(&mut tb);
        }

        let net_latest = Arc::new(Mutex::new(NetSnapshot::default()));
        let net_latest_bg = Arc::clone(&net_latest);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop_flag);

        // Background nettop poller so UI/process sampling never blocks on subprocess I/O
        thread::spawn(move || {
            while !stop_bg.load(Ordering::Relaxed) {
                if let Some(entries) = poll_nettop() {
                    if let Ok(mut guard) = net_latest_bg.lock() {
                        guard.entries = entries;
                    }
                }
                for _ in 0..5 {
                    if stop_bg.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(90));
                }
            }
        });

        let mut mon = Self {
            timebase_numer: tb.numer.max(1) as u64,
            timebase_denom: tb.denom.max(1) as u64,
            prev_cpu: HashMap::new(),
            prev_sample_time: Instant::now(),
            meta_cache: HashMap::new(),
            net_latest,
            net_prev: HashMap::new(),
            net_rates: HashMap::new(),
            stop_flag,
        };
        // Prime initial CPU/disk baseline
        let _ = mon.sample();
        mon
    }

    fn mach_to_ns(&self, ticks: u64) -> u64 {
        ((ticks as u128 * self.timebase_numer as u128) / self.timebase_denom as u128) as u64
    }

    pub fn sample(&mut self) -> Vec<ProcessInfo> {
        let now = Instant::now();
        let dt_ns = now
            .duration_since(self.prev_sample_time)
            .as_nanos()
            .max(1_000_000) as f64;
        let dt_s = (dt_ns / 1e9).max(0.001);
        self.prev_sample_time = now;

        // Update network rates from latest nettop snapshot
        if let Ok(guard) = self.net_latest.lock() {
            if !guard.entries.is_empty() {
                for (&pid, (_, bin_ref, bout_ref)) in &guard.entries {
                    let bin = *bin_ref;
                    let bout = *bout_ref;
                    if let Some(prev) = self.net_prev.get(&pid) {
                        let ndt = now.duration_since(prev.at).as_secs_f64();
                        if ndt >= 0.2 {
                            let rx_bps = bin.saturating_sub(prev.bytes_in) as f64 / ndt;
                            let tx_bps = bout.saturating_sub(prev.bytes_out) as f64 / ndt;
                            self.net_rates.insert(pid, (rx_bps, tx_bps, bin, bout));
                            self.net_prev.insert(
                                pid,
                                NetPrev {
                                    bytes_in: bin,
                                    bytes_out: bout,
                                    at: now,
                                },
                            );
                        } else if let Some(r) = self.net_rates.get_mut(&pid) {
                            r.2 = bin;
                            r.3 = bout;
                        }
                    } else {
                        self.net_prev.insert(
                            pid,
                            NetPrev {
                                bytes_in: bin,
                                bytes_out: bout,
                                at: now,
                            },
                        );
                        self.net_rates.insert(pid, (0.0, 0.0, bin, bout));
                    }
                }
            }
        }

        let pids = list_all_pids();
        let mut next_prev: HashMap<u32, PrevCpuDisk> = HashMap::with_capacity(pids.len());
        let mut result = Vec::with_capacity(pids.len());

        let net_names: HashMap<u32, String> = self
            .net_latest
            .lock()
            .map(|g| {
                g.entries
                    .iter()
                    .map(|(&pid, (name, _, _))| (pid, name.clone()))
                    .collect()
            })
            .unwrap_or_default();

        for pid in pids {
            if pid <= 0 {
                if pid == 0 {
                    // kernel_task
                    let (rx, tx, bin, bout) =
                        self.net_rates.get(&0).copied().unwrap_or((0.0, 0.0, 0, 0));
                    result.push(ProcessInfo {
                        pid: 0,
                        ppid: 0,
                        uid: 0,
                        name: "kernel_task".to_string(),
                        app_name: None,
                        is_app: false,
                        cpu_pct: 0.0,
                        mem_bytes: 0,
                        net_rx_bps: rx,
                        net_tx_bps: tx,
                        net_rx_bytes: bin,
                        net_tx_bytes: bout,
                        net_total_bytes: bin.saturating_add(bout),
                        disk_read_bps: 0.0,
                        disk_write_bps: 0.0,
                        threads: 0,
                    });
                }
                continue;
            }
            let pid_u32 = pid as u32;

            let bsd = get_bsd_info(pid);
            let task = get_task_info(pid);
            let rusage = get_rusage_v4(pid);

            if bsd.is_none() && task.is_none() && rusage.is_none() {
                continue;
            }

            let start_tvsec = bsd.map(|b| b.pbi_start_tvsec).unwrap_or(0);
            let ppid = bsd.map(|b| b.pbi_ppid).unwrap_or(0);
            let uid = bsd.map(|b| b.pbi_uid).unwrap_or(0);

            let meta = if let Some(cached) = self
                .meta_cache
                .get(&pid_u32)
                .filter(|m| m.start_tvsec == start_tvsec)
            {
                cached.clone()
            } else {
                let path = get_pid_path(pid);
                let bsd_name = bsd.map(|b| extract_bsd_name(&b)).unwrap_or_default();
                let (app_name, is_app) = path
                    .as_deref()
                    .map(parse_app_bundle)
                    .unwrap_or((None, false));

                let fallback_name = if !bsd_name.is_empty() {
                    bsd_name
                } else if let Some(ref p) = path {
                    p.rsplit('/').next().unwrap_or(p).to_string()
                } else if let Some(nn) = net_names.get(&pid_u32) {
                    nn.clone()
                } else {
                    format!("pid {}", pid_u32)
                };

                let m = CachedMeta {
                    start_tvsec,
                    name: fallback_name,
                    app_name,
                    is_app,
                };
                self.meta_cache.insert(pid_u32, m.clone());
                m
            };

            let cpu_ns = if let Some(r) = rusage {
                self.mach_to_ns(r.ri_user_time.saturating_add(r.ri_system_time))
            } else if let Some(t) = task {
                self.mach_to_ns(t.pti_total_user.saturating_add(t.pti_total_system))
            } else {
                0
            };

            let mem_bytes = if let Some(r) = rusage {
                if r.ri_phys_footprint > 0 {
                    r.ri_phys_footprint
                } else {
                    r.ri_resident_size
                }
            } else if let Some(t) = task {
                t.pti_resident_size
            } else {
                0
            };

            let (disk_read, disk_write) = rusage
                .map(|r| (r.ri_diskio_bytesread, r.ri_diskio_byteswritten))
                .unwrap_or((0, 0));

            let threads = task.map(|t| t.pti_threadnum).unwrap_or(0);

            let (cpu_pct, disk_read_bps, disk_write_bps) = if let Some(prev) =
                self.prev_cpu.get(&pid_u32).filter(|p| p.start_tvsec == start_tvsec)
            {
                let d_cpu = cpu_ns.saturating_sub(prev.cpu_ns) as f64;
                let cpu = (d_cpu / dt_ns) * 100.0;
                let dr = disk_read.saturating_sub(prev.disk_read) as f64 / dt_s;
                let dw = disk_write.saturating_sub(prev.disk_write) as f64 / dt_s;
                (cpu, dr, dw)
            } else {
                (0.0, 0.0, 0.0)
            };

            next_prev.insert(
                pid_u32,
                PrevCpuDisk {
                    cpu_ns,
                    disk_read,
                    disk_write,
                    start_tvsec,
                },
            );

            let (net_rx_bps, net_tx_bps, net_rx_bytes, net_tx_bytes) =
                self.net_rates.get(&pid_u32).copied().unwrap_or((0.0, 0.0, 0, 0));

            result.push(ProcessInfo {
                pid: pid_u32,
                ppid,
                uid,
                name: meta.name,
                app_name: meta.app_name,
                is_app: meta.is_app,
                cpu_pct,
                mem_bytes,
                net_rx_bps,
                net_tx_bps,
                net_rx_bytes,
                net_tx_bytes,
                net_total_bytes: net_rx_bytes.saturating_add(net_tx_bytes),
                disk_read_bps,
                disk_write_bps,
                threads,
            });
        }

        // Prune dead PIDs from caches
        self.meta_cache.retain(|pid, _| next_prev.contains_key(pid));
        self.net_prev.retain(|pid, _| *pid == 0 || next_prev.contains_key(pid));
        self.net_rates.retain(|pid, _| *pid == 0 || next_prev.contains_key(pid));
        self.prev_cpu = next_prev;

        result
    }
}

impl Drop for ProcessMonitor {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

pub fn sort_processes(
    procs: &mut [ProcessInfo],
    sort_col: SortColumn,
    descending: bool,
) {
    procs.sort_by(|a, b| {
        let ord = match sort_col {
            SortColumn::Cpu => a
                .cpu_pct
                .partial_cmp(&b.cpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.mem_bytes.cmp(&b.mem_bytes)),
            SortColumn::Memory => a
                .mem_bytes
                .cmp(&b.mem_bytes)
                .then_with(|| {
                    a.cpu_pct
                        .partial_cmp(&b.cpu_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                }),
            SortColumn::NetDown => a
                .net_rx_bps
                .partial_cmp(&b.net_rx_bps)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.net_rx_bytes.cmp(&b.net_rx_bytes))
                .then_with(|| {
                    a.net_tx_bps
                        .partial_cmp(&b.net_tx_bps)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.mem_bytes.cmp(&b.mem_bytes)),
            SortColumn::NetUp => a
                .net_tx_bps
                .partial_cmp(&b.net_tx_bps)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.net_tx_bytes.cmp(&b.net_tx_bytes))
                .then_with(|| {
                    a.net_rx_bps
                        .partial_cmp(&b.net_rx_bps)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.mem_bytes.cmp(&b.mem_bytes)),
            SortColumn::App => {
                // Sort Apps first when ascending? Actually alphabetical by display_name (case-insensitive)
                // with apps grouped or plain alphabetical like Task Manager Name column
                a.display_name()
                    .to_ascii_lowercase()
                    .cmp(&b.display_name().to_ascii_lowercase())
                    .then_with(|| a.pid.cmp(&b.pid))
            }
            SortColumn::Disk => a
                .disk_bps()
                .partial_cmp(&b.disk_bps())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.mem_bytes.cmp(&b.mem_bytes)),
            SortColumn::Pid => a.pid.cmp(&b.pid),
            SortColumn::Threads => a
                .threads
                .cmp(&b.threads)
                .then_with(|| a.mem_bytes.cmp(&b.mem_bytes)),
        };
        if descending {
            ord.reverse()
        } else {
            ord
        }
    });
}

fn list_all_pids() -> Vec<i32> {
    let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    let cap = (count as usize) + 64;
    let mut buf = vec![0i32; cap];
    let bytes = unsafe {
        proc_listallpids(
            buf.as_mut_ptr() as *mut libc::c_void,
            (cap * std::mem::size_of::<i32>()) as i32,
        )
    };
    if bytes <= 0 {
        return Vec::new();
    }
    let n = (bytes as usize / std::mem::size_of::<i32>()).min(cap);
    buf.truncate(n);
    buf
}

fn get_bsd_info(pid: i32) -> Option<ProcBsdInfoRaw> {
    let mut info: ProcBsdInfoRaw = unsafe { std::mem::zeroed() };
    let sz = std::mem::size_of::<ProcBsdInfoRaw>() as i32;
    let ret = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            sz,
        )
    };
    if ret == sz {
        Some(info)
    } else {
        None
    }
}

fn get_task_info(pid: i32) -> Option<ProcTaskInfoRaw> {
    let mut info: ProcTaskInfoRaw = unsafe { std::mem::zeroed() };
    let sz = std::mem::size_of::<ProcTaskInfoRaw>() as i32;
    let ret = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            sz,
        )
    };
    if ret == sz {
        Some(info)
    } else {
        None
    }
}

fn get_rusage_v4(pid: i32) -> Option<RusageInfoV4Raw> {
    let mut info: RusageInfoV4Raw = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        proc_pid_rusage(
            pid,
            RUSAGE_INFO_V4,
            &mut info as *mut _ as *mut libc::c_void,
        )
    };
    if ret == 0 {
        Some(info)
    } else {
        None
    }
}

fn get_pid_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let ret = unsafe {
        proc_pidpath(
            pid,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as u32,
        )
    };
    if ret <= 0 {
        return None;
    }
    let len = (ret as usize).min(buf.len());
    let s = String::from_utf8_lossy(&buf[..len]).trim_end_matches('\0').to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn extract_bsd_name(bsd: &ProcBsdInfoRaw) -> String {
    let n = unsafe { CStr::from_ptr(bsd.pbi_name.as_ptr() as *const libc::c_char) }
        .to_string_lossy()
        .trim()
        .to_string();
    if !n.is_empty() {
        return n;
    }
    unsafe { CStr::from_ptr(bsd.pbi_comm.as_ptr() as *const libc::c_char) }
        .to_string_lossy()
        .trim()
        .to_string()
}

fn parse_app_bundle(path: &str) -> (Option<String>, bool) {
    // Check if executable is inside a .app bundle
    if let Some(idx) = path.find(".app/Contents/") {
        let bundle_prefix = &path[..idx];
        let bundle_name = bundle_prefix.rsplit('/').next().unwrap_or(bundle_prefix);
        let is_main = path[idx + ".app/Contents/".len()..].starts_with("MacOS/");
        return (Some(bundle_name.to_string()), is_main);
    }
    (None, false)
}

fn poll_nettop() -> Option<HashMap<u32, (String, u64, u64)>> {
    let out = Command::new("nettop")
        .args(["-P", "-L", "1", "-n", "-x", "-J", "bytes_in,bytes_out"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map = HashMap::new();
    for line in text.lines().skip(1) {
        let mut parts = line.split(',');
        let Some(name_pid) = parts.next() else { continue };
        let Some(in_str) = parts.next() else { continue };
        let Some(out_str) = parts.next() else { continue };
        let Some((name, pid_str)) = name_pid.rsplit_once('.') else { continue };
        let Ok(pid) = pid_str.trim().parse::<u32>() else { continue };
        let bytes_in = in_str.trim().parse::<u64>().unwrap_or(0);
        let bytes_out = out_str.trim().parse::<u64>().unwrap_or(0);
        map.insert(pid, (name.to_string(), bytes_in, bytes_out));
    }
    Some(map)
}

pub fn terminate_process(pid: u32) -> std::io::Result<()> {
    if pid == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "cannot terminate kernel_task",
        ));
    }
    let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::SortColumn;

    #[test]
    fn sort_column_cycle_matches_screen_order() {
        let screen_order = [
            SortColumn::Pid,
            SortColumn::App,
            SortColumn::Cpu,
            SortColumn::Memory,
            SortColumn::NetDown,
            SortColumn::NetUp,
            SortColumn::Disk,
            SortColumn::Threads,
        ];
        for i in 0..screen_order.len() {
            let cur = screen_order[i];
            let expected_next = screen_order[(i + 1) % screen_order.len()];
            let expected_prev = screen_order[(i + screen_order.len() - 1) % screen_order.len()];
            assert_eq!(cur.next(), expected_next);
            assert_eq!(cur.prev(), expected_prev);
        }
    }
}

