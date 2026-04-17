//! System resource monitor that polls CPU, RAM, disks, network, and GPU
//! usage at a regular interval and emits the snapshot as a Tauri event.
//!
//! The heavy lifting is done by the `sysinfo` crate (cross-platform) plus
//! Windows-specific GPU usage queries via the D3DKMT kernel thunk.
//!
//! Architecture mirrors `updater.rs`: a single named thread, fire-and-forget
//! via `start_sysmonitor(app)`, all errors are non-fatal warnings.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
pub const SYSMONITOR_EVENT: &str = "app:sysmonitor";

// ── Payload types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
pub struct SystemSnapshot {
    pub cpu_usage: f32,
    pub ram_total_mb: u64,
    pub ram_used_mb: u64,
    pub ram_usage: f32,
    pub gpus: Vec<GpuInfo>,
    pub disks: Vec<DiskInfo>,
    pub net: NetInfo,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct GpuInfo {
    pub name: String,
    pub usage: f32,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DiskInfo {
    pub name: String,
    pub total_gb: f64,
    pub used_gb: f64,
    pub usage: f32,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct NetInfo {
    pub upload_bytes_sec: u64,
    pub download_bytes_sec: u64,
}

// ── Public entry point ─────────────────────────────────────────────────────

pub fn start_sysmonitor(app: AppHandle) {
    std::thread::Builder::new()
        .name("ymux-sysmonitor".into())
        .spawn(move || monitor_loop(app))
        .expect("spawn sysmonitor thread");
}

// ── Monitor loop ───────────────────────────────────────────────────────────

fn monitor_loop(app: AppHandle) {
    use sysinfo::{Disks, Networks, System};

    let mut sys = System::new_all();
    let mut networks = Networks::new_with_refreshed_list();
    let disks = Disks::new_with_refreshed_list();

    // GPU poller (Windows only, no-op on other platforms).
    #[cfg(windows)]
    let mut gpu_poller = gpu::GpuPoller::new();

    // Give the system a moment to gather baseline data.
    std::thread::sleep(Duration::from_millis(500));
    sys.refresh_all();

    loop {
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        networks.refresh(true);

        let cpu_usage = sys.global_cpu_usage();
        let ram_total_mb = sys.total_memory() / (1024 * 1024);
        let ram_used_mb = sys.used_memory() / (1024 * 1024);
        let ram_usage = if ram_total_mb > 0 {
            (ram_used_mb as f32 / ram_total_mb as f32) * 100.0
        } else {
            0.0
        };

        let disk_infos: Vec<DiskInfo> = disks
            .iter()
            .filter(|d| d.total_space() > 0)
            .map(|d| {
                let total = d.total_space() as f64 / (1024.0 * 1024.0 * 1024.0);
                let available = d.available_space() as f64 / (1024.0 * 1024.0 * 1024.0);
                let used = total - available;
                let usage = if total > 0.0 {
                    (used / total * 100.0) as f32
                } else {
                    0.0
                };
                let mount = d.mount_point().to_string_lossy().to_string();
                DiskInfo {
                    name: if mount.is_empty() {
                        d.name().to_string_lossy().to_string()
                    } else {
                        mount
                    },
                    total_gb: (total * 10.0).round() / 10.0,
                    used_gb: (used * 10.0).round() / 10.0,
                    usage,
                }
            })
            .collect();

        // Network: sum across all interfaces, compute delta per second.
        let mut total_rx: u64 = 0;
        let mut total_tx: u64 = 0;
        for (_name, data) in networks.iter() {
            total_rx += data.received();
            total_tx += data.transmitted();
        }
        let interval_secs = POLL_INTERVAL.as_secs().max(1);
        let net = NetInfo {
            download_bytes_sec: total_rx / interval_secs,
            upload_bytes_sec: total_tx / interval_secs,
        };

        #[cfg(windows)]
        let gpus = gpu_poller.poll();
        #[cfg(not(windows))]
        let gpus: Vec<GpuInfo> = Vec::new();

        let snapshot = SystemSnapshot {
            cpu_usage,
            ram_total_mb,
            ram_used_mb,
            ram_usage,
            gpus,
            disks: disk_infos,
            net,
        };

        if let Err(e) = app.emit(SYSMONITOR_EVENT, &snapshot) {
            tracing::warn!(error = %e, "emit sysmonitor snapshot failed");
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

// ── Windows GPU usage via D3DKMT ───────────────────────────────────────────

#[cfg(windows)]
mod gpu {
    use super::GpuInfo;

    /// Attempts to read GPU engine utilization via the Windows D3DKMT kernel
    /// interface. Falls back to an empty vec on any failure — GPU stats are
    /// best-effort.
    pub struct GpuPoller {
        adapters: Vec<AdapterEntry>,
    }

    struct AdapterEntry {
        name: String,
        handle: u32,
    }

    impl GpuPoller {
        pub fn new() -> Self {
            let adapters = enumerate_adapters();
            Self { adapters }
        }

        pub fn poll(&mut self) -> Vec<GpuInfo> {
            self.adapters
                .iter()
                .filter_map(|a| {
                    let usage = query_engine_usage(a.handle).unwrap_or(-1.0);
                    if usage < 0.0 {
                        return None;
                    }
                    Some(GpuInfo {
                        name: a.name.clone(),
                        usage: usage as f32,
                    })
                })
                .collect()
        }
    }

    fn enumerate_adapters() -> Vec<AdapterEntry> {
        // Use the Windows registry to discover display adapters. This is
        // simpler than calling D3DKMTEnumAdapters (which needs linking to
        // gdi32). The registry path contains the adapter description.
        use windows::Win32::System::Registry::*;
        use windows::core::PCWSTR;

        let mut result = Vec::new();
        let subkey = wide_string("SYSTEM\\CurrentControlSet\\Control\\Video");
        let mut hkey = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(subkey.as_ptr()), 0, KEY_READ, &mut hkey)
        };
        if status.is_err() {
            return result;
        }

        let mut idx = 0u32;
        loop {
            let mut name_buf = [0u16; 512];
            let mut name_len = name_buf.len() as u32;
            let status = unsafe {
                RegEnumKeyExW(
                    hkey,
                    idx,
                    windows::core::PWSTR(name_buf.as_mut_ptr()),
                    &mut name_len,
                    None,
                    windows::core::PWSTR::null(),
                    None,
                    None,
                )
            };
            if status.is_err() {
                break;
            }
            idx += 1;

            // Each GUID subkey has a "0000" child containing "DriverDesc".
            let sub = format!(
                "SYSTEM\\CurrentControlSet\\Control\\Video\\{}\\0000",
                String::from_utf16_lossy(&name_buf[..name_len as usize])
            );
            let sub_wide = wide_string(&sub);
            let mut sub_hkey = HKEY::default();
            let ok = unsafe {
                RegOpenKeyExW(
                    HKEY_LOCAL_MACHINE,
                    PCWSTR(sub_wide.as_ptr()),
                    0,
                    KEY_READ,
                    &mut sub_hkey,
                )
            };
            if ok.is_err() {
                continue;
            }
            let mut data_buf = [0u8; 1024];
            let mut data_len = data_buf.len() as u32;
            let mut kind = REG_VALUE_TYPE(0);
            let val_name = wide_string("DriverDesc");
            let qok = unsafe {
                RegQueryValueExW(
                    sub_hkey,
                    PCWSTR(val_name.as_ptr()),
                    None,
                    Some(&mut kind),
                    Some(data_buf.as_mut_ptr()),
                    Some(&mut data_len),
                )
            };
            let _ = unsafe { RegCloseKey(sub_hkey) };
            if qok.is_err() || kind != REG_SZ {
                continue;
            }
            let desc = String::from_utf16_lossy(
                &data_buf[..data_len as usize]
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect::<Vec<u16>>(),
            )
            .trim_end_matches('\0')
            .to_string();
            if desc.is_empty() {
                continue;
            }
            // Deduplicate by name.
            if result.iter().any(|a: &AdapterEntry| a.name == desc) {
                continue;
            }
            result.push(AdapterEntry {
                name: desc,
                handle: idx - 1,
            });
        }
        let _ = unsafe { RegCloseKey(hkey) };
        result
    }

    fn query_engine_usage(_handle: u32) -> Result<f64, ()> {
        // GPU utilization via D3DKMT requires linking to gdi32.dll at runtime
        // and is quite involved. For the MVP, we return a sentinel to indicate
        // "not available". The frontend can hide the GPU section gracefully.
        //
        // A future enhancement can use PDH counters:
        // "\GPU Engine(*)\Utilization Percentage" (available on Win10 1709+).
        Err(())
    }

    fn wide_string(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_serialises_to_json() {
        let snap = SystemSnapshot {
            cpu_usage: 42.5,
            ram_total_mb: 16384,
            ram_used_mb: 8192,
            ram_usage: 50.0,
            gpus: vec![GpuInfo {
                name: "RTX 4090".into(),
                usage: 75.0,
            }],
            disks: vec![DiskInfo {
                name: "C:\\".into(),
                total_gb: 500.0,
                used_gb: 250.0,
                usage: 50.0,
            }],
            net: NetInfo {
                upload_bytes_sec: 1024,
                download_bytes_sec: 2048,
            },
        };
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("42.5"));
        assert!(json.contains("RTX 4090"));
    }
}
