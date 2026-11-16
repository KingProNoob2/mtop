use anyhow::Result;
use std::ffi::{CStr, CString};
use std::ptr;

#[derive(Debug, Clone)]
pub struct ChipInfo {
    pub chip_name: String,
    pub os_version: String,
    pub p_cores: u32,
    pub e_cores: u32,
    pub gpu_cores: u32,
    pub mem_total_bytes: u64,
    pub ecpu_freqs_mhz: Vec<f64>,
    pub pcpu_freqs_mhz: Vec<f64>,
    pub gpu_freqs_mhz: Vec<f64>,
}

impl ChipInfo {
    pub fn collect() -> Result<Self> {
        let chip_name = sysctl_string("machdep.cpu.brand_string")
            .unwrap_or_else(|_| "Apple Silicon".to_string());
        let os_version = sysctl_string("kern.osproductversion")
            .unwrap_or_else(|_| "macOS".to_string());
        let p_cores = sysctl_u32("hw.perflevel0.physicalcpu").unwrap_or(0);
        let e_cores = sysctl_u32("hw.perflevel1.physicalcpu").unwrap_or(0);
        let mem_total_bytes = sysctl_u64("hw.memsize").unwrap_or(0);

        let gpu_cores = read_gpu_core_count().unwrap_or(0);
        let (ecpu_freqs_mhz, pcpu_freqs_mhz, gpu_freqs_mhz) = read_pmgr_frequencies();

        Ok(Self {
            chip_name,
            os_version,
            p_cores,
            e_cores,
            gpu_cores,
            mem_total_bytes,
            ecpu_freqs_mhz,
            pcpu_freqs_mhz,
            gpu_freqs_mhz,
        })
    }
}

pub fn sysctl_string(name: &str) -> Result<String> {
    let cname = CString::new(name)?;
    let mut size: libc::size_t = 0;
    let ret = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            ptr::null_mut(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if ret != 0 || size == 0 {
        anyhow::bail!("sysctlbyname({}) failed", name);
    }
    let mut buf = vec![0u8; size];
    let ret = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if ret != 0 {
        anyhow::bail!("sysctlbyname({}) read failed", name);
    }
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr() as *const libc::c_char) };
    Ok(cstr.to_string_lossy().trim().to_string())
}

pub fn sysctl_u32(name: &str) -> Result<u32> {
    let cname = CString::new(name)?;
    let mut val: u32 = 0;
    let mut size = std::mem::size_of::<u32>();
    let ret = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            &mut val as *mut u32 as *mut libc::c_void,
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if ret != 0 {
        anyhow::bail!("sysctlbyname({}) failed", name);
    }
    Ok(val)
}

pub fn sysctl_u64(name: &str) -> Result<u64> {
    let cname = CString::new(name)?;
    let mut val: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    let ret = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            &mut val as *mut u64 as *mut libc::c_void,
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if ret != 0 {
        anyhow::bail!("sysctlbyname({}) failed", name);
    }
    Ok(val)
}

// Minimal CoreFoundation / IOKit FFI for reading pmgr and AGXAccelerator properties.
type CFTypeRef = *const libc::c_void;
type CFAllocatorRef = *const libc::c_void;
type CFStringRef = *const libc::c_void;
type CFDataRef = *const libc::c_void;
type CFNumberRef = *const libc::c_void;
type CFDictionaryRef = *const libc::c_void;
type CFMutableDictionaryRef = *mut libc::c_void;
type IoObjectT = u32;
type IoRegistryEntryT = u32;

unsafe extern "C" {
    static kCFAllocatorDefault: CFAllocatorRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDataGetLength(theData: CFDataRef) -> isize;
    fn CFDataGetBytePtr(theData: CFDataRef) -> *const u8;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: CFNumberRef, theType: i32, valuePtr: *mut libc::c_void) -> bool;
    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        cStr: *const libc::c_char,
        encoding: u32,
    ) -> CFStringRef;

    fn IOServiceMatching(name: *const libc::c_char) -> CFMutableDictionaryRef;
    fn IOServiceNameMatching(name: *const libc::c_char) -> CFMutableDictionaryRef;
    fn IOServiceGetMatchingService(mainPort: u32, matching: CFDictionaryRef) -> IoObjectT;
    fn IORegistryEntryCreateCFProperty(
        entry: IoRegistryEntryT,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        options: u32,
    ) -> CFTypeRef;
    fn IOObjectRelease(object: IoObjectT) -> i32;
}

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT64_TYPE: i32 = 4;

fn cfstr(s: &str) -> CFStringRef {
    let cs = CString::new(s).unwrap();
    unsafe { CFStringCreateWithCString(kCFAllocatorDefault, cs.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
}

fn read_gpu_core_count() -> Option<u32> {
    let class_name = CString::new("AGXAccelerator").ok()?;
    let matching = unsafe { IOServiceMatching(class_name.as_ptr()) };
    if matching.is_null() {
        return None;
    }
    let service = unsafe { IOServiceGetMatchingService(0, matching) };
    if service == 0 {
        return None;
    }
    let key = cfstr("gpu-core-count");
    let prop = unsafe { IORegistryEntryCreateCFProperty(service, key, kCFAllocatorDefault, 0) };
    unsafe {
        CFRelease(key);
        IOObjectRelease(service);
    }
    if prop.is_null() {
        return None;
    }
    let mut val: i64 = 0;
    let ok = unsafe {
        if CFGetTypeID(prop) == CFNumberGetTypeID() {
            CFNumberGetValue(prop, K_CF_NUMBER_SINT64_TYPE, &mut val as *mut i64 as *mut libc::c_void)
        } else {
            false
        }
    };
    unsafe { CFRelease(prop) };
    if ok && val > 0 {
        Some(val as u32)
    } else {
        None
    }
}

fn read_pmgr_frequencies() -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let name = CString::new("pmgr").unwrap();
    let matching = unsafe { IOServiceNameMatching(name.as_ptr()) };
    if matching.is_null() {
        return (vec![], vec![], vec![]);
    }
    let service = unsafe { IOServiceGetMatchingService(0, matching) };
    if service == 0 {
        return (vec![], vec![], vec![]);
    }

    let ecpu = read_voltage_states(service, &["voltage-states1-sram", "voltage-states1"], false);
    let pcpu = read_voltage_states(service, &["voltage-states5-sram", "voltage-states5"], false);
    // GPU voltage-states9 has 0 as first entry (OFF state), so keep_zero=false filters it out or we handle it
    let gpu = read_voltage_states(service, &["voltage-states9-sram", "voltage-states9"], false);

    unsafe { IOObjectRelease(service) };
    (ecpu, pcpu, gpu)
}

fn read_voltage_states(service: IoObjectT, keys: &[&str], keep_zero: bool) -> Vec<f64> {
    for k in keys {
        let key = cfstr(k);
        let prop = unsafe { IORegistryEntryCreateCFProperty(service, key, kCFAllocatorDefault, 0) };
        unsafe { CFRelease(key) };
        if prop.is_null() {
            continue;
        }
        let is_data = unsafe { CFGetTypeID(prop) == CFDataGetTypeID() };
        if !is_data {
            unsafe { CFRelease(prop) };
            continue;
        }
        let len = unsafe { CFDataGetLength(prop) } as usize;
        let ptr = unsafe { CFDataGetBytePtr(prop) };
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        let mut freqs = Vec::new();
        for chunk in bytes.chunks_exact(8) {
            let raw = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if raw == 0 && !keep_zero {
                continue;
            }
            let mhz = raw_freq_to_mhz(raw);
            freqs.push(mhz);
        }
        unsafe { CFRelease(prop) };
        if !freqs.is_empty() {
            return freqs;
        }
    }
    Vec::new()
}

fn raw_freq_to_mhz(raw: u32) -> f64 {
    if raw == 0 {
        0.0
    } else if raw > 10_000_000 {
        raw as f64 / 1_000_000.0
    } else if raw > 10_000 {
        raw as f64 / 1_000.0
    } else {
        raw as f64
    }
}

pub fn get_thermal_pressure() -> String {
    unsafe extern "C" {
        fn notify_register_check(name: *const libc::c_char, out_token: *mut i32) -> u32;
        fn notify_get_state(token: i32, state64: *mut u64) -> u32;
        fn notify_cancel(token: i32) -> u32;
    }
    let cname = CString::new("com.apple.system.thermalpressurelevel").unwrap();
    let mut token: i32 = 0;
    let status = unsafe { notify_register_check(cname.as_ptr(), &mut token) };
    if status != 0 {
        return "Unknown".to_string();
    }
    let mut level: u64 = 0;
    let st2 = unsafe { notify_get_state(token, &mut level) };
    unsafe { notify_cancel(token) };
    if st2 != 0 {
        return "Unknown".to_string();
    }
    match level {
        0 => "Nominal".to_string(),
        1 => "Moderate".to_string(),
        2 => "Heavy".to_string(),
        3 => "Trapping".to_string(),
        4 => "Sleeping".to_string(),
        _ => format!("Level {}", level),
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub active_bytes: u64,
    pub wired_bytes: u64,
    pub compressed_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

pub fn read_memory_stats(total_bytes: u64) -> MemoryStats {
    let mut stats = MemoryStats {
        total_bytes,
        ..Default::default()
    };

    unsafe {
        let page_size = libc::vm_page_size;
        let mut vm_stat: libc::vm_statistics64_data_t = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::vm_statistics64_data_t>()
            / std::mem::size_of::<libc::natural_t>()) as libc::mach_msg_type_number_t;
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        let kr = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            &mut vm_stat as *mut _ as libc::host_info64_t,
            &mut count,
        );
        if kr == libc::KERN_SUCCESS {
            let ps = page_size as u64;
            stats.active_bytes = vm_stat.active_count as u64 * ps;
            stats.wired_bytes = vm_stat.wire_count as u64 * ps;
            stats.compressed_bytes = vm_stat.compressor_page_count as u64 * ps;
            // App + Wired + Compressed (matches Activity Monitor's "Memory Used" closely)
            let internal = vm_stat.internal_page_count as u64 * ps;
            let purgeable = vm_stat.purgeable_count as u64 * ps;
            let app = internal.saturating_sub(purgeable);
            stats.used_bytes = app + stats.wired_bytes + stats.compressed_bytes;
        }

        // Swap usage via sysctl vm.swapusage
        #[repr(C)]
        struct XswUsage {
            xsu_total: u64,
            xsu_avail: u64,
            xsu_used: u64,
            xsu_pagesize: u32,
            xsu_encrypted: bool,
        }
        let cname = CString::new("vm.swapusage").unwrap();
        let mut xsw: XswUsage = std::mem::zeroed();
        let mut size = std::mem::size_of::<XswUsage>();
        if libc::sysctlbyname(
            cname.as_ptr(),
            &mut xsw as *mut _ as *mut libc::c_void,
            &mut size,
            ptr::null_mut(),
            0,
        ) == 0
        {
            stats.swap_total_bytes = xsw.xsu_total;
            stats.swap_used_bytes = xsw.xsu_used;
        }
    }

    stats
}
