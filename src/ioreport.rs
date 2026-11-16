use anyhow::Result;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::ptr;
use std::time::Instant;

type CFTypeRef = *const libc::c_void;
type CFAllocatorRef = *const libc::c_void;
type CFStringRef = *const libc::c_void;
type CFDictionaryRef = *const libc::c_void;
type CFMutableDictionaryRef = *mut libc::c_void;
type CFArrayRef = *const libc::c_void;
type IOReportSubscriptionRef = *const libc::c_void;

unsafe extern "C" {
    static kCFAllocatorDefault: CFAllocatorRef;
    static kCFTypeDictionaryKeyCallBacks: libc::c_void;
    static kCFTypeDictionaryValueCallBacks: libc::c_void;

    fn CFRelease(cf: CFTypeRef);
    fn CFDictionaryCreateMutable(
        allocator: CFAllocatorRef,
        capacity: isize,
        keyCallBacks: *const libc::c_void,
        valueCallBacks: *const libc::c_void,
    ) -> CFMutableDictionaryRef;
    fn CFDictionaryGetValue(theDict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    fn CFArrayGetCount(theArray: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(theArray: CFArrayRef, idx: isize) -> CFTypeRef;
    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        cStr: *const libc::c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFStringGetCString(
        theString: CFStringRef,
        buffer: *mut libc::c_char,
        bufferSize: isize,
        encoding: u32,
    ) -> bool;

    fn IOReportCopyChannelsInGroup(
        group: CFStringRef,
        subgroup: CFStringRef,
        a: u64,
        b: u64,
        c: u64,
    ) -> CFDictionaryRef;
    fn IOReportMergeChannels(
        dst: CFMutableDictionaryRef,
        src: CFDictionaryRef,
        nil_arg: CFTypeRef,
    );
    fn IOReportCreateSubscription(
        a: *mut libc::c_void,
        desiredChannels: CFMutableDictionaryRef,
        subbedChannels: *mut CFMutableDictionaryRef,
        channel_id: u64,
        b: CFTypeRef,
    ) -> IOReportSubscriptionRef;
    fn IOReportCreateSamples(
        sub: IOReportSubscriptionRef,
        channels: CFMutableDictionaryRef,
        nil_arg: CFTypeRef,
    ) -> CFDictionaryRef;
    fn IOReportCreateSamplesDelta(
        prev: CFDictionaryRef,
        next: CFDictionaryRef,
        nil_arg: CFTypeRef,
    ) -> CFDictionaryRef;

    fn IOReportChannelGetGroup(item: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetSubGroup(item: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetChannelName(item: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetUnitLabel(item: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetFormat(item: CFDictionaryRef) -> i32;
    fn IOReportSimpleGetIntegerValue(item: CFDictionaryRef, err: *mut i32) -> i64;
    fn IOReportStateGetCount(item: CFDictionaryRef) -> i32;
    fn IOReportStateGetNameForIndex(item: CFDictionaryRef, idx: i32) -> CFStringRef;
    fn IOReportStateGetResidency(item: CFDictionaryRef, idx: i32) -> i64;
}

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

fn make_cfstr(s: &str) -> CFStringRef {
    let cs = CString::new(s).unwrap();
    unsafe { CFStringCreateWithCString(kCFAllocatorDefault, cs.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
}

fn read_cfstr(s: CFStringRef) -> String {
    if s.is_null() {
        return String::new();
    }
    let mut buf = [0i8; 256];
    let ok = unsafe { CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, K_CF_STRING_ENCODING_UTF8) };
    if !ok {
        return String::new();
    }
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_string()
}

#[derive(Debug, Clone, Default)]
pub struct CoreStat {
    pub name: String,
    pub util_pct: f64,
    pub freq_mhz: f64,
}

#[derive(Debug, Clone, Default)]
pub struct IOReportSample {
    pub ecpu_util_pct: f64,
    pub ecpu_freq_mhz: f64,
    pub pcpu_util_pct: f64,
    pub pcpu_freq_mhz: f64,
    pub ecpu_cores: Vec<CoreStat>,
    pub pcpu_cores: Vec<CoreStat>,
    pub gpu_util_pct: f64,
    pub gpu_freq_mhz: f64,
    pub gpu_power_w: Option<f64>,
    pub cpu_power_w: Option<f64>,
    pub ane_power_w: Option<f64>,
    pub dram_power_w: Option<f64>,
}

pub struct IOReportMonitor {
    sub: IOReportSubscriptionRef,
    subbed_channels: CFMutableDictionaryRef,
    prev_sample: CFDictionaryRef,
    prev_time: Instant,
    ecpu_freqs: Vec<f64>,
    pcpu_freqs: Vec<f64>,
    gpu_freqs: Vec<f64>,
}

unsafe impl Send for IOReportMonitor {}

impl IOReportMonitor {
    pub fn new(ecpu_freqs: Vec<f64>, pcpu_freqs: Vec<f64>, gpu_freqs: Vec<f64>) -> Result<Self> {
        unsafe {
            let channels = CFDictionaryCreateMutable(
                kCFAllocatorDefault,
                0,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            if channels.is_null() {
                anyhow::bail!("Failed to create CFMutableDictionary for IOReport");
            }

            for grp in ["Energy Model", "CPU Stats", "GPU Stats"] {
                let gstr = make_cfstr(grp);
                let g = IOReportCopyChannelsInGroup(gstr, ptr::null(), 0, 0, 0);
                CFRelease(gstr);
                if !g.is_null() {
                    IOReportMergeChannels(channels, g, ptr::null());
                    CFRelease(g);
                }
            }

            let mut subbed: CFMutableDictionaryRef = ptr::null_mut();
            let sub = IOReportCreateSubscription(ptr::null_mut(), channels, &mut subbed, 0, ptr::null());
            CFRelease(channels);

            if sub.is_null() || subbed.is_null() {
                anyhow::bail!("IOReportCreateSubscription failed");
            }

            let prev_sample = IOReportCreateSamples(sub, subbed, ptr::null());
            if prev_sample.is_null() {
                CFRelease(sub);
                CFRelease(subbed);
                anyhow::bail!("IOReportCreateSamples failed");
            }

            Ok(Self {
                sub,
                subbed_channels: subbed,
                prev_sample,
                prev_time: Instant::now(),
                ecpu_freqs,
                pcpu_freqs,
                gpu_freqs,
            })
        }
    }

    pub fn sample(&mut self) -> Result<IOReportSample> {
        unsafe {
            let now_sample = IOReportCreateSamples(self.sub, self.subbed_channels, ptr::null());
            let now_time = Instant::now();
            if now_sample.is_null() {
                anyhow::bail!("IOReportCreateSamples returned null");
            }

            let dt = now_time.duration_since(self.prev_time).as_secs_f64().max(0.001);
            let delta = IOReportCreateSamplesDelta(self.prev_sample, now_sample, ptr::null());
            CFRelease(self.prev_sample);
            self.prev_sample = now_sample;
            self.prev_time = now_time;

            if delta.is_null() {
                anyhow::bail!("IOReportCreateSamplesDelta returned null");
            }

            let mut out = IOReportSample::default();
            let mut ecores: BTreeMap<usize, CoreStat> = BTreeMap::new();
            let mut pcores: BTreeMap<usize, CoreStat> = BTreeMap::new();

            let key = make_cfstr("IOReportChannels");
            let arr = CFDictionaryGetValue(delta, key);
            CFRelease(key);

            if !arr.is_null() {
                let count = CFArrayGetCount(arr);
                for i in 0..count {
                    let item = CFArrayGetValueAtIndex(arr, i);
                    if item.is_null() {
                        continue;
                    }
                    let grp = read_cfstr(IOReportChannelGetGroup(item));
                    let subg = read_cfstr(IOReportChannelGetSubGroup(item));
                    let ch = read_cfstr(IOReportChannelGetChannelName(item));
                    let fmt = IOReportChannelGetFormat(item);

                    if grp == "Energy Model" && fmt == 1 {
                        let mut err = 0i32;
                        let val = IOReportSimpleGetIntegerValue(item, &mut err);
                        if err == 0 && val > 0 {
                            let unit = read_cfstr(IOReportChannelGetUnitLabel(item));
                            let joules = match unit.as_str() {
                                "nJ" => val as f64 / 1e9,
                                "uJ" => val as f64 / 1e6,
                                "mJ" => val as f64 / 1e3,
                                _ => val as f64 / 1e3,
                            };
                            let watts = joules / dt;
                            if ch == "GPU Energy" || ch == "GPU" {
                                out.gpu_power_w = Some(watts);
                            } else if ch == "CPU Energy" {
                                out.cpu_power_w = Some(watts);
                            } else if ch == "ANE" {
                                out.ane_power_w = Some(watts);
                            } else if ch == "DRAM" {
                                out.dram_power_w = Some(watts);
                            }
                        }
                    } else if grp == "CPU Stats" && fmt == 2 {
                        if subg == "CPU Complex Performance States" {
                            if ch == "ECPU" {
                                let (u, f) = calc_state_util_freq(item, &self.ecpu_freqs);
                                out.ecpu_util_pct = u;
                                out.ecpu_freq_mhz = f;
                            } else if ch == "PCPU" {
                                let (u, f) = calc_state_util_freq(item, &self.pcpu_freqs);
                                out.pcpu_util_pct = u;
                                out.pcpu_freq_mhz = f;
                            }
                        } else if subg == "CPU Core Performance States" {
                            if let Some(idx_str) = ch.strip_prefix("ECPU") {
                                if let Ok(idx) = idx_str.parse::<usize>() {
                                    let (u, f) = calc_state_util_freq(item, &self.ecpu_freqs);
                                    ecores.insert(
                                        idx,
                                        CoreStat {
                                            name: format!("E{}", idx),
                                            util_pct: u,
                                            freq_mhz: f,
                                        },
                                    );
                                }
                            } else if let Some(idx_str) = ch.strip_prefix("PCPU") {
                                if let Ok(idx) = idx_str.parse::<usize>() {
                                    let (u, f) = calc_state_util_freq(item, &self.pcpu_freqs);
                                    pcores.insert(
                                        idx,
                                        CoreStat {
                                            name: format!("P{}", idx),
                                            util_pct: u,
                                            freq_mhz: f,
                                        },
                                    );
                                }
                            }
                        }
                    } else if grp == "GPU Stats" && fmt == 2 && subg == "GPU Performance States" && ch == "GPUPH" {
                        let (u, f) = calc_state_util_freq(item, &self.gpu_freqs);
                        out.gpu_util_pct = u;
                        out.gpu_freq_mhz = f;
                    }
                }
            }

            CFRelease(delta);

            out.ecpu_cores = ecores.into_values().collect();
            out.pcpu_cores = pcores.into_values().collect();

            Ok(out)
        }
    }
}

impl Drop for IOReportMonitor {
    fn drop(&mut self) {
        unsafe {
            if !self.prev_sample.is_null() {
                CFRelease(self.prev_sample);
            }
            if !self.subbed_channels.is_null() {
                CFRelease(self.subbed_channels);
            }
            if !self.sub.is_null() {
                CFRelease(self.sub);
            }
        }
    }
}

unsafe fn calc_state_util_freq(item: CFDictionaryRef, freqs_mhz: &[f64]) -> (f64, f64) {
    let count = unsafe { IOReportStateGetCount(item) };
    if count <= 0 {
        return (0.0, 0.0);
    }
    let mut idle_res: i64 = 0;
    let mut active_res: Vec<i64> = Vec::new();
    let mut total_active: i64 = 0;

    for i in 0..count {
        let sname = read_cfstr(unsafe { IOReportStateGetNameForIndex(item, i) });
        let res = unsafe { IOReportStateGetResidency(item, i) }.max(0);
        if sname == "IDLE" || sname == "OFF" || sname == "DOWN" {
            idle_res += res;
        } else {
            total_active += res;
            active_res.push(res);
        }
    }

    let total = idle_res + total_active;
    if total <= 0 {
        return (0.0, 0.0);
    }

    let util = (total_active as f64 / total as f64) * 100.0;
    let freq = if total_active > 0 && !freqs_mhz.is_empty() {
        let n = active_res.len().min(freqs_mhz.len());
        let weighted: f64 = active_res[..n]
            .iter()
            .zip(freqs_mhz[..n].iter())
            .map(|(&r, &f)| r as f64 * f)
            .sum();
        weighted / total_active as f64
    } else {
        0.0
    };

    (util.clamp(0.0, 100.0), freq)
}
