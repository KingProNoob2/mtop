use crate::ioreport::{CoreStat, IOReportMonitor};
use crate::powermetrics::PowermetricsRunner;
use crate::smc::SmcReader;
use crate::sysinfo::{get_thermal_pressure, read_memory_stats, read_network_bytes, ChipInfo};
use anyhow::Result;
use serde::Serialize;
use std::collections::VecDeque;
use std::time::Instant;

const HISTORY_LEN: usize = 60;

#[derive(Debug, Clone, Serialize)]
pub struct CoreSnapshot {
    pub name: String,
    pub util_pct: f64,
    pub freq_mhz: f64,
}

impl From<CoreStat> for CoreSnapshot {
    fn from(c: CoreStat) -> Self {
        Self {
            name: c.name,
            util_pct: c.util_pct,
            freq_mhz: c.freq_mhz,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricsSnapshot {
    pub ecpu_util_pct: f64,
    pub ecpu_freq_mhz: f64,
    pub pcpu_util_pct: f64,
    pub pcpu_freq_mhz: f64,
    pub cpu_total_util_pct: f64,
    pub ecpu_cores: Vec<CoreSnapshot>,
    pub pcpu_cores: Vec<CoreSnapshot>,

    pub gpu_util_pct: f64,
    pub gpu_freq_mhz: f64,

    pub cpu_power_w: Option<f64>,
    pub gpu_power_w: Option<f64>,
    pub ane_power_w: Option<f64>,
    pub dram_power_w: Option<f64>,
    pub package_power_w: Option<f64>,
    pub system_power_w: Option<f64>,

    pub soc_temp_avg_c: Option<f64>,
    pub soc_temp_max_c: Option<f64>,
    pub gpu_temp_avg_c: Option<f64>,
    pub thermal_pressure: String,

    pub mem_total_bytes: u64,
    pub mem_used_bytes: u64,
    pub mem_active_bytes: u64,
    pub mem_wired_bytes: u64,
    pub mem_compressed_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,

    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub net_rx_total_bytes: u64,
    pub net_tx_total_bytes: u64,
}

pub struct MetricsCollector {
    pub chip: ChipInfo,
    ioreport: IOReportMonitor,
    smc: Option<SmcReader>,
    powermetrics: Option<PowermetricsRunner>,

    pub ecpu_hist: VecDeque<u64>,
    pub pcpu_hist: VecDeque<u64>,
    pub gpu_hist: VecDeque<u64>,
    pub power_hist: VecDeque<u64>,
    pub mem_hist: VecDeque<u64>,
    pub net_hist: VecDeque<u64>,

    pub peak_system_power_w: f64,
    pub avg_system_power_w: f64,
    power_samples: u64,
    power_sum_w: f64,

    prev_net_rx: u64,
    prev_net_tx: u64,
    prev_net_at: Instant,
    pub peak_net_rx_bps: f64,
    pub peak_net_tx_bps: f64,
}

impl MetricsCollector {
    pub fn new(interval_ms: u64, use_powermetrics: bool) -> Result<Self> {
        let chip = ChipInfo::collect()?;
        let ioreport = IOReportMonitor::new(
            chip.ecpu_freqs_mhz.clone(),
            chip.pcpu_freqs_mhz.clone(),
            chip.gpu_freqs_mhz.clone(),
        )?;
        let smc = SmcReader::open().ok();
        let is_root = unsafe { libc::geteuid() == 0 };
        let powermetrics = if is_root || use_powermetrics {
            PowermetricsRunner::start(interval_ms)
        } else {
            None
        };

        let (init_rx, init_tx) = read_network_bytes();

        Ok(Self {
            chip,
            ioreport,
            smc,
            powermetrics,
            ecpu_hist: VecDeque::from(vec![0; HISTORY_LEN]),
            pcpu_hist: VecDeque::from(vec![0; HISTORY_LEN]),
            gpu_hist: VecDeque::from(vec![0; HISTORY_LEN]),
            power_hist: VecDeque::from(vec![0; HISTORY_LEN]),
            mem_hist: VecDeque::from(vec![0; HISTORY_LEN]),
            net_hist: VecDeque::from(vec![0; HISTORY_LEN]),
            peak_system_power_w: 0.0,
            avg_system_power_w: 0.0,
            power_samples: 0,
            power_sum_w: 0.0,
            prev_net_rx: init_rx,
            prev_net_tx: init_tx,
            prev_net_at: Instant::now(),
            peak_net_rx_bps: 0.0,
            peak_net_tx_bps: 0.0,
        })
    }

    pub fn has_powermetrics(&self) -> bool {
        self.powermetrics.is_some()
    }

    pub fn collect(&mut self) -> Result<MetricsSnapshot> {
        let ir = self.ioreport.sample()?;
        let smc = self.smc.as_ref().map(|s| s.sample()).unwrap_or_default();
        let pm = self.powermetrics.as_ref().and_then(|p| p.latest());
        let mem = read_memory_stats(self.chip.mem_total_bytes);
        let mut thermal_pressure = get_thermal_pressure();

        let mut cpu_power_w = ir.cpu_power_w;
        let mut gpu_power_w = ir.gpu_power_w;
        let mut ane_power_w = ir.ane_power_w;
        let mut dram_power_w = ir.dram_power_w;
        let mut package_power_w = None;

        if let Some(pm_data) = pm {
            cpu_power_w = pm_data.cpu_power_w.or(cpu_power_w);
            gpu_power_w = gpu_power_w.or(pm_data.gpu_power_w);
            ane_power_w = pm_data.ane_power_w.or(ane_power_w);
            dram_power_w = pm_data.dram_power_w.or(dram_power_w);
            package_power_w = pm_data.package_power_w;
            if let Some(tp) = pm_data.thermal_pressure {
                thermal_pressure = tp;
            }
        }

        let system_power_w = smc.system_power_w.or(smc.dc_power_w).or(package_power_w);

        let e_cnt = ir.ecpu_cores.len().max(1) as f64;
        let p_cnt = ir.pcpu_cores.len().max(1) as f64;
        let cpu_total_util_pct =
            (ir.ecpu_util_pct * e_cnt + ir.pcpu_util_pct * p_cnt) / (e_cnt + p_cnt);

        push_hist(&mut self.ecpu_hist, ir.ecpu_util_pct.round() as u64);
        push_hist(&mut self.pcpu_hist, ir.pcpu_util_pct.round() as u64);
        push_hist(&mut self.gpu_hist, ir.gpu_util_pct.round() as u64);

        if let Some(pw) = system_power_w {
            if pw > self.peak_system_power_w {
                self.peak_system_power_w = pw;
            }
            self.power_samples += 1;
            self.power_sum_w += pw;
            self.avg_system_power_w = self.power_sum_w / self.power_samples as f64;
            push_hist(&mut self.power_hist, (pw * 10.0).round() as u64);
        }

        let mem_pct = if mem.total_bytes > 0 {
            ((mem.used_bytes as f64 / mem.total_bytes as f64) * 100.0).round() as u64
        } else {
            0
        };
        push_hist(&mut self.mem_hist, mem_pct);

        let (net_rx_total_bytes, net_tx_total_bytes) = read_network_bytes();
        let net_now = Instant::now();
        let net_dt = net_now
            .duration_since(self.prev_net_at)
            .as_secs_f64()
            .max(0.001);
        let net_rx_bps = net_rx_total_bytes.saturating_sub(self.prev_net_rx) as f64 / net_dt;
        let net_tx_bps = net_tx_total_bytes.saturating_sub(self.prev_net_tx) as f64 / net_dt;
        self.prev_net_rx = net_rx_total_bytes;
        self.prev_net_tx = net_tx_total_bytes;
        self.prev_net_at = net_now;
        if net_rx_bps > self.peak_net_rx_bps {
            self.peak_net_rx_bps = net_rx_bps;
        }
        if net_tx_bps > self.peak_net_tx_bps {
            self.peak_net_tx_bps = net_tx_bps;
        }
        let net_kbs = ((net_rx_bps + net_tx_bps) / 1024.0).round() as u64;
        push_hist(&mut self.net_hist, net_kbs);

        Ok(MetricsSnapshot {
            ecpu_util_pct: ir.ecpu_util_pct,
            ecpu_freq_mhz: ir.ecpu_freq_mhz,
            pcpu_util_pct: ir.pcpu_util_pct,
            pcpu_freq_mhz: ir.pcpu_freq_mhz,
            cpu_total_util_pct,
            ecpu_cores: ir.ecpu_cores.into_iter().map(Into::into).collect(),
            pcpu_cores: ir.pcpu_cores.into_iter().map(Into::into).collect(),
            gpu_util_pct: ir.gpu_util_pct,
            gpu_freq_mhz: ir.gpu_freq_mhz,
            cpu_power_w,
            gpu_power_w,
            ane_power_w,
            dram_power_w,
            package_power_w,
            system_power_w,
            soc_temp_avg_c: smc.soc_temp_avg_c,
            soc_temp_max_c: smc.soc_temp_max_c,
            gpu_temp_avg_c: smc.gpu_temp_avg_c,
            thermal_pressure,
            mem_total_bytes: mem.total_bytes,
            mem_used_bytes: mem.used_bytes,
            mem_active_bytes: mem.active_bytes,
            mem_wired_bytes: mem.wired_bytes,
            mem_compressed_bytes: mem.compressed_bytes,
            swap_total_bytes: mem.swap_total_bytes,
            swap_used_bytes: mem.swap_used_bytes,
            net_rx_bps,
            net_tx_bps,
            net_rx_total_bytes,
            net_tx_total_bytes,
        })
    }
}

fn push_hist(q: &mut VecDeque<u64>, val: u64) {
    if q.len() >= HISTORY_LEN {
        q.pop_front();
    }
    q.push_back(val);
}
