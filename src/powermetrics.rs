use std::io::{BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, Default)]
pub struct PowermetricsData {
    pub cpu_power_w: Option<f64>,
    pub gpu_power_w: Option<f64>,
    pub ane_power_w: Option<f64>,
    pub dram_power_w: Option<f64>,
    pub package_power_w: Option<f64>,
    pub thermal_pressure: Option<String>,
}

pub struct PowermetricsRunner {
    child: Option<Child>,
    latest: Arc<Mutex<Option<PowermetricsData>>>,
}

impl PowermetricsRunner {
    pub fn start(interval_ms: u64) -> Option<Self> {
        let mut child = Command::new("powermetrics")
            .args([
                "-i",
                &interval_ms.to_string(),
                "-s",
                "cpu_power,gpu_power,ane_power,thermal",
                "-f",
                "plist",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let stdout = child.stdout.take()?;
        let latest = Arc::new(Mutex::new(None));
        let latest_clone = Arc::clone(&latest);

        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::with_capacity(64 * 1024);
            let mut byte = [0u8; 4096];
            loop {
                match reader.read(&mut byte) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for &b in &byte[..n] {
                            if b == 0 {
                                if !buf.is_empty() {
                                    if let Some(parsed) = parse_plist_chunk(&buf) {
                                        if let Ok(mut guard) = latest_clone.lock() {
                                            *guard = Some(parsed);
                                        }
                                    }
                                    buf.clear();
                                }
                            } else {
                                buf.push(b);
                            }
                        }
                    }
                }
            }
        });

        Some(Self {
            child: Some(child),
            latest,
        })
    }

    pub fn latest(&self) -> Option<PowermetricsData> {
        self.latest.lock().ok().and_then(|g| g.clone())
    }
}

impl Drop for PowermetricsRunner {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn parse_plist_chunk(data: &[u8]) -> Option<PowermetricsData> {
    let val = plist::Value::from_reader_xml(data).ok()?;
    let dict = val.as_dictionary()?;
    let mut out = PowermetricsData::default();

    if let Some(tp) = dict.get("thermal_pressure").and_then(|v| v.as_string()) {
        out.thermal_pressure = Some(tp.to_string());
    }

    if let Some(proc_dict) = dict.get("processor").and_then(|v| v.as_dictionary()) {
        // powermetrics reports energy in mJ over the interval or power in mW depending on field
        if let Some(mw) = proc_dict.get("cpu_power").and_then(as_f64) {
            out.cpu_power_w = Some(mw / 1000.0);
        }
        if let Some(mw) = proc_dict.get("gpu_power").and_then(as_f64) {
            out.gpu_power_w = Some(mw / 1000.0);
        }
        if let Some(mw) = proc_dict.get("ane_power").and_then(as_f64) {
            out.ane_power_w = Some(mw / 1000.0);
        }
        if let Some(mw) = proc_dict.get("dram_power").and_then(as_f64) {
            out.dram_power_w = Some(mw / 1000.0);
        }
        if let Some(mw) = proc_dict.get("combined_power").and_then(as_f64) {
            out.package_power_w = Some(mw / 1000.0);
        } else if let Some(mw) = proc_dict.get("package_watts").and_then(as_f64) {
            out.package_power_w = Some(mw);
        }
    }

    Some(out)
}

fn as_f64(v: &plist::Value) -> Option<f64> {
    v.as_real()
        .or_else(|| v.as_signed_integer().map(|i| i as f64))
        .or_else(|| v.as_unsigned_integer().map(|u| u as f64))
}
