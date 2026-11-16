use anyhow::Result;
use std::ffi::CString;

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct SMCKeyDataVers {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct SMCKeyDataPLimitData {
    version: u16,
    length: u16,
    cpu_plimit: u32,
    gpu_plimit: u32,
    mem_plimit: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct SMCKeyDataKeyInfo {
    data_size: u32,
    data_type: u32,
    data_attributes: u8,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct SMCKeyData {
    key: u32,
    vers: SMCKeyDataVers,
    p_limit_data: SMCKeyDataPLimitData,
    key_info: SMCKeyDataKeyInfo,
    result: u8,
    status: u8,
    data8: u8,
    data32: u32,
    bytes: [u8; 32],
}

unsafe extern "C" {
    fn IOServiceMatching(name: *const libc::c_char) -> *mut libc::c_void;
    fn IOServiceGetMatchingService(mainPort: u32, matching: *const libc::c_void) -> u32;
    fn IOServiceOpen(service: u32, owningTask: u32, type_: u32, connect: *mut u32) -> i32;
    fn IOServiceClose(connect: u32) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOConnectCallStructMethod(
        connection: u32,
        selector: u32,
        inputStruct: *const libc::c_void,
        inputStructCnt: usize,
        outputStruct: *mut libc::c_void,
        outputStructCnt: *mut usize,
    ) -> i32;
    fn mach_task_self() -> u32;
}

const K_SMC_READ_KEY: u8 = 5;
const K_SMC_GET_KEY_FROM_INDEX: u8 = 8;
const K_SMC_GET_KEY_INFO: u8 = 9;

const fn str_to_key(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
}

fn key_to_str(k: u32) -> [u8; 4] {
    [
        ((k >> 24) & 0xff) as u8,
        ((k >> 16) & 0xff) as u8,
        ((k >> 8) & 0xff) as u8,
        (k & 0xff) as u8,
    ]
}

#[derive(Debug, Clone, Default)]
pub struct SmcSample {
    pub system_power_w: Option<f64>,
    pub dc_power_w: Option<f64>,
    pub soc_temp_avg_c: Option<f64>,
    pub soc_temp_max_c: Option<f64>,
    pub gpu_temp_avg_c: Option<f64>,
}

pub struct SmcReader {
    conn: u32,
    cpu_temp_keys: Vec<(u32, u32)>, // (key, data_size)
    gpu_temp_keys: Vec<(u32, u32)>,
}

unsafe impl Send for SmcReader {}

impl SmcReader {
    pub fn open() -> Result<Self> {
        unsafe {
            let name = CString::new("AppleSMC")?;
            let matching = IOServiceMatching(name.as_ptr());
            if matching.is_null() {
                anyhow::bail!("IOServiceMatching(AppleSMC) failed");
            }
            let svc = IOServiceGetMatchingService(0, matching);
            if svc == 0 {
                anyhow::bail!("AppleSMC service not found");
            }
            let mut conn: u32 = 0;
            let kr = IOServiceOpen(svc, mach_task_self(), 0, &mut conn);
            IOObjectRelease(svc);
            if kr != 0 || conn == 0 {
                anyhow::bail!("IOServiceOpen(AppleSMC) failed: 0x{:x}", kr);
            }

            let mut reader = Self {
                conn,
                cpu_temp_keys: Vec::new(),
                gpu_temp_keys: Vec::new(),
            };
            reader.discover_temp_keys();
            Ok(reader)
        }
    }

    fn call(&self, input: &SMCKeyData) -> Option<SMCKeyData> {
        let mut output = SMCKeyData::default();
        let mut out_sz = std::mem::size_of::<SMCKeyData>();
        let kr = unsafe {
            IOConnectCallStructMethod(
                self.conn,
                2,
                input as *const _ as *const libc::c_void,
                std::mem::size_of::<SMCKeyData>(),
                &mut output as *mut _ as *mut libc::c_void,
                &mut out_sz,
            )
        };
        if kr == 0 && output.result == 0 {
            Some(output)
        } else {
            None
        }
    }

    fn read_flt(&self, key: u32) -> Option<f64> {
        let mut inp = SMCKeyData::default();
        inp.key = key;
        inp.data8 = K_SMC_GET_KEY_INFO;
        let info = self.call(&inp)?;
        if info.key_info.data_type != str_to_key(b"flt ") || info.key_info.data_size != 4 {
            return None;
        }
        self.read_flt_with_size(key, 4)
    }

    fn read_flt_with_size(&self, key: u32, size: u32) -> Option<f64> {
        let mut inp = SMCKeyData::default();
        inp.key = key;
        inp.data8 = K_SMC_READ_KEY;
        inp.key_info.data_size = size;
        let out = self.call(&inp)?;
        let val = f32::from_le_bytes([out.bytes[0], out.bytes[1], out.bytes[2], out.bytes[3]]) as f64;
        if val.is_finite() {
            Some(val)
        } else {
            None
        }
    }

    fn discover_temp_keys(&mut self) {
        let mut inp = SMCKeyData::default();
        inp.key = str_to_key(b"#KEY");
        inp.data8 = K_SMC_GET_KEY_INFO;
        let Some(info) = self.call(&inp) else { return };
        inp.data8 = K_SMC_READ_KEY;
        inp.key_info.data_size = info.key_info.data_size;
        let Some(out) = self.call(&inp) else { return };
        let count = u32::from_be_bytes([out.bytes[0], out.bytes[1], out.bytes[2], out.bytes[3]]);

        for i in 0..count.min(4096) {
            let mut idx_in = SMCKeyData::default();
            idx_in.data8 = K_SMC_GET_KEY_FROM_INDEX;
            idx_in.data32 = i;
            let Some(idx_out) = self.call(&idx_in) else { continue };
            let k = idx_out.key;
            let kb = key_to_str(k);
            // Die temperature sensors on Apple Silicon: Tp* (PMU/CPU die), TC* (CPU), Tg* (GPU)
            let is_cpu = kb[0] == b'T' && (kb[1] == b'p' || kb[1] == b'C');
            let is_gpu = kb[0] == b'T' && kb[1] == b'g';
            if !is_cpu && !is_gpu {
                continue;
            }
            let mut ki = SMCKeyData::default();
            ki.key = k;
            ki.data8 = K_SMC_GET_KEY_INFO;
            let Some(kinfo) = self.call(&ki) else { continue };
            if kinfo.key_info.data_type == str_to_key(b"flt ") && kinfo.key_info.data_size == 4 {
                if let Some(v) = self.read_flt_with_size(k, 4) {
                    if (10.0..=125.0).contains(&v) {
                        if is_gpu {
                            self.gpu_temp_keys.push((k, 4));
                        } else {
                            self.cpu_temp_keys.push((k, 4));
                        }
                    }
                }
            }
        }
    }

    pub fn sample(&self) -> SmcSample {
        let system_power_w = self.read_flt(str_to_key(b"PSTR")).filter(|&v| (0.0..500.0).contains(&v));
        let dc_power_w = self.read_flt(str_to_key(b"PDTR")).filter(|&v| (0.0..500.0).contains(&v));

        let cpu_vals: Vec<f64> = self
            .cpu_temp_keys
            .iter()
            .filter_map(|&(k, sz)| self.read_flt_with_size(k, sz))
            .filter(|&v| (10.0..=125.0).contains(&v))
            .collect();
        let gpu_vals: Vec<f64> = self
            .gpu_temp_keys
            .iter()
            .filter_map(|&(k, sz)| self.read_flt_with_size(k, sz))
            .filter(|&v| (10.0..=125.0).contains(&v))
            .collect();

        let soc_temp_avg_c = if !cpu_vals.is_empty() {
            Some(cpu_vals.iter().sum::<f64>() / cpu_vals.len() as f64)
        } else {
            None
        };
        let soc_temp_max_c = cpu_vals.iter().copied().reduce(f64::max);
        let gpu_temp_avg_c = if !gpu_vals.is_empty() {
            Some(gpu_vals.iter().sum::<f64>() / gpu_vals.len() as f64)
        } else {
            None
        };

        SmcSample {
            system_power_w,
            dc_power_w,
            soc_temp_avg_c,
            soc_temp_max_c,
            gpu_temp_avg_c,
        }
    }
}

impl Drop for SmcReader {
    fn drop(&mut self) {
        if self.conn != 0 {
            unsafe {
                IOServiceClose(self.conn);
            }
        }
    }
}
