//! Windows: obciążenie karty Intel z liczników wydajności „GPU Engine” (te same, z których
//! korzysta Menedżer zadań), filtrowane do procesów ffmpeg uruchomionych przez videopipeline.
//! Do tego CPU (GetSystemTimes) i RAM (GlobalMemoryStatusEx). Bez zewnętrznych crate'ów —
//! bezpośrednio pdh.dll i kernel32.dll. Nazwy liczników angielskie (PdhAddEnglishCounterW),
//! więc działa też na polskim Windows.
#![allow(non_snake_case)]
use crate::intelmon::IntelStats;
use std::{collections::HashMap, ffi::c_void, path::Path, ptr};

type Handle = *mut c_void;

const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
const PDH_MORE_DATA: u32 = 0x8000_07D2;

#[repr(C)]
struct PdhFmtCounterValue {
    c_status: u32,
    value: f64, // unia PDH_FMT_COUNTERVALUE — przy PDH_FMT_DOUBLE pole doubleValue
}

#[repr(C)]
struct PdhFmtCounterValueItemW {
    name: *mut u16,
    value: PdhFmtCounterValue,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FileTime {
    lo: u32,
    hi: u32,
}

#[repr(C)]
#[derive(Default)]
struct MemoryStatusEx {
    length: u32,
    memory_load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[link(name = "pdh")]
extern "system" {
    fn PdhOpenQueryW(data_source: *const u16, user_data: usize, query: *mut Handle) -> u32;
    fn PdhAddEnglishCounterW(query: Handle, path: *const u16, user_data: usize, counter: *mut Handle) -> u32;
    fn PdhCollectQueryData(query: Handle) -> u32;
    fn PdhGetFormattedCounterArrayW(
        counter: Handle,
        format: u32,
        buffer_size: *mut u32,
        item_count: *mut u32,
        items: *mut PdhFmtCounterValueItemW,
    ) -> u32;
    fn PdhCloseQuery(query: Handle) -> u32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

/// (total, idle) w jednostkach 100 ns — jak (total, idle) z /proc/stat na Linuksie.
pub fn read_cpu() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default());
    // SAFETY: wskaźniki na lokalne struktury o układzie FILETIME.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let v = |t: FileTime| ((t.hi as u64) << 32) | t.lo as u64;
    // Czas jądra zawiera już czas bezczynności.
    Some((v(kernel) + v(user), v(idle)))
}

/// (całkowita, dostępna) pamięć w bajtach.
pub fn read_mem() -> Option<(u64, u64)> {
    let mut m = MemoryStatusEx { length: std::mem::size_of::<MemoryStatusEx>() as u32, ..Default::default() };
    // SAFETY: struktura MEMORYSTATUSEX z ustawionym polem dwLength.
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return None;
    }
    Some((m.total_phys, m.avail_phys))
}

/// Liczniki GPU Engine / GPU Process Memory dla procesów ffmpeg.
pub struct GpuEngineSampler {
    query: Handle,
    engine: Handle,
    dedicated: Handle,
    shared: Handle,
    primed: bool,
}

// Uchwyty PDH można używać z dowolnego wątku (byle nie równocześnie — mamy &mut self).
unsafe impl Send for GpuEngineSampler {}

impl GpuEngineSampler {
    pub fn new(_dev: Option<&Path>) -> Self {
        let mut s = GpuEngineSampler {
            query: ptr::null_mut(),
            engine: ptr::null_mut(),
            dedicated: ptr::null_mut(),
            shared: ptr::null_mut(),
            primed: false,
        };
        // SAFETY: wywołania PDH z poprawnymi wskaźnikami wyjściowymi; błędy zostawiają null.
        unsafe {
            if PdhOpenQueryW(ptr::null(), 0, &mut s.query) != 0 {
                s.query = ptr::null_mut();
                return s;
            }
            let add = |path: &str, out: &mut Handle| {
                let w = wide(path);
                if PdhAddEnglishCounterW(s.query, w.as_ptr(), 0, out) != 0 {
                    *out = ptr::null_mut();
                }
            };
            let (mut e, mut d, mut sh) = (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
            add("\\GPU Engine(*)\\Utilization Percentage", &mut e);
            add("\\GPU Process Memory(*)\\Dedicated Usage", &mut d);
            add("\\GPU Process Memory(*)\\Shared Usage", &mut sh);
            s.engine = e;
            s.dedicated = d;
            s.shared = sh;
            PdhCollectQueryData(s.query); // pierwszy odczyt — liczniki procentowe potrzebują dwóch
        }
        s
    }

    /// Wszystkie instancje licznika: (nazwa instancji, wartość).
    fn values(counter: Handle) -> Vec<(String, f64)> {
        if counter.is_null() {
            return vec![];
        }
        let (mut size, mut count) = (0u32, 0u32);
        // SAFETY: pierwsze wywołanie z pustym buforem zwraca wymagany rozmiar w bajtach;
        // drugie wypełnia bufor wyrównany do 8 bajtów (Vec<u64>).
        unsafe {
            let r = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE | PDH_FMT_NOCAP100,
                &mut size,
                &mut count,
                ptr::null_mut(),
            );
            if r != PDH_MORE_DATA || size == 0 {
                return vec![];
            }
            let mut buf: Vec<u64> = vec![0; (size as usize).div_ceil(8) + 1];
            let items = buf.as_mut_ptr() as *mut PdhFmtCounterValueItemW;
            if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, items)
                != 0
            {
                return vec![];
            }
            (0..count as usize)
                .map(|i| {
                    let it = &*items.add(i);
                    (from_wide(it.name), it.value.value)
                })
                .filter(|(_, v)| v.is_finite())
                .collect()
        }
    }

    pub fn sample(&mut self, pids: &[u32]) -> IntelStats {
        let mut st = IntelStats { name: "Intel GPU (Windows · liczniki GPU Engine)".into(), ..Default::default() };
        if self.query.is_null() {
            return st;
        }
        // SAFETY: uchwyt zapytania otwarty w new().
        unsafe { PdhCollectQueryData(self.query) };
        if !self.primed {
            self.primed = true;
            return st; // pierwszy przyrost dopiero przy następnym odczycie
        }

        // Instancja: pid_1234_luid_0x..._0x..._phys_0_eng_3_engtype_VideoDecode
        let pid_of = |name: &str| -> Option<u32> {
            name.strip_prefix("pid_")?.split('_').next()?.parse().ok()
        };
        let mut per_engine: HashMap<(String, String), f64> = HashMap::new(); // (silnik, typ) → suma po procesach
        for (name, v) in Self::values(self.engine) {
            let Some(pid) = pid_of(&name) else { continue };
            if !pids.contains(&pid) {
                continue;
            }
            let Some((head, ty)) = name.split_once("_engtype_") else { continue };
            let engine = head.split_once("_luid_").map(|(_, e)| e.to_string()).unwrap_or_default();
            *per_engine.entry((engine, ty.to_string())).or_default() += v;
        }
        // Na typ silnika: najbardziej obciążona instancja (jak w Menedżerze zadań).
        let mut by_type: HashMap<String, f64> = HashMap::new();
        for ((_, ty), v) in per_engine {
            let e = by_type.entry(ty).or_default();
            *e = e.max(v);
        }
        let get = |pred: &dyn Fn(&str) -> bool| -> f64 {
            by_type.iter().filter(|(t, _)| pred(t)).map(|(_, v)| *v).fold(0.0, f64::max).clamp(0.0, 100.0)
        };
        // Na Windows koder i dekoder mogą być osobnymi silnikami (VideoEncode / VideoDecode / Video Codec).
        st.video = Some(get(&|t| t.starts_with("Video") && t != "VideoProcessing"));
        st.enhance = Some(get(&|t| t == "VideoProcessing"));
        st.render = Some(get(&|t| t == "3D" || t == "Compute" || t.starts_with("Compute")));

        let mut mem = 0.0;
        let mut any = false;
        for c in [self.dedicated, self.shared] {
            for (name, v) in Self::values(c) {
                if pid_of(&name).is_some_and(|p| pids.contains(&p)) {
                    mem += v;
                    any = true;
                }
            }
        }
        st.mem_bytes = any.then_some(mem as u64);
        st
    }
}

impl Drop for GpuEngineSampler {
    fn drop(&mut self) {
        if !self.query.is_null() {
            // SAFETY: uchwyt z PdhOpenQueryW, zamykany raz.
            unsafe { PdhCloseQuery(self.query) };
        }
    }
}
