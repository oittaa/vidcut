use serde_json::{Value, json};

pub fn version() -> String {
    #[cfg(windows)]
    {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let build = windows::text(key, "CurrentBuild").unwrap_or_default();
        let revision = windows::number(key, "UBR").unwrap_or(0);
        format!("windows {build}.{revision}")
    }
    #[cfg(target_os = "macos")]
    {
        let version = macos::sysctl(c"kern.osproductversion").unwrap_or_default();
        let build = macos::sysctl(c"kern.osversion").unwrap_or_default();
        format!("macos {version} ({build})")
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn gpus() -> Vec<Value> {
    #[cfg(windows)]
    {
        windows::adapters()
            .into_iter()
            .map(|(name, driver)| json!({ "name": name, "driver": driver }))
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        pollster::block_on(instance.enumerate_adapters(wgpu::Backends::METAL))
            .iter()
            .map(|a| json!({ "name": a.get_info().name }))
            .collect()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let nvidia = std::fs::read_to_string("/sys/module/nvidia/version")
            .ok()
            .map(|v| v.trim().to_string());
        vidcut_av::export::encode::render_nodes()
            .into_iter()
            .map(|(node, driver)| {
                let version = (driver == "nvidia").then(|| nvidia.clone()).flatten();
                json!({ "node": node, "driver": driver, "version": version })
            })
            .collect()
    }
}

pub fn disk(path: &std::path::Path) -> Option<vidcut_core::disk_cache::Disk> {
    let path = path.ancestors().find(|p| p.exists())?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: an all-zero `statvfs` is valid, and statvfs writes only the one given.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `name` is a NUL-terminated path and `s` a live `statvfs`.
        if unsafe { libc::statvfs(name.as_ptr(), &mut s) } != 0 {
            return None;
        }
        let unit = s.f_frsize as u64;
        Some(vidcut_core::disk_cache::Disk {
            size: (s.f_blocks as u64).saturating_mul(unit),
            free: (s.f_bavail as u64).saturating_mul(unit),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let (mut free, mut size) = (0u64, 0u64);
        // SAFETY: `name` is a NUL-terminated path; the counts are live u64s, the third may be null.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                name.as_ptr(),
                &mut free,
                &mut size,
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(vidcut_core::disk_cache::Disk { size, free })
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };

    const DISPLAY: &str =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn text(key: &str, value: &str) -> Option<String> {
        let (key, value) = (wide(key), wide(value));
        let mut buf = [0u16; 512];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: NUL-terminated names; `buf` holds the `size` bytes the value may fill.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != 0 {
            return None;
        }
        let len = (size as usize / 2).min(buf.len());
        let text = String::from_utf16_lossy(&buf[..len]);
        Some(text.trim_end_matches('\0').to_string())
    }

    pub fn number(key: &str, value: &str) -> Option<u32> {
        let (key, value) = (wide(key), wide(value));
        let mut n = 0u32;
        let mut size = 4u32;
        // SAFETY: NUL-terminated names; the value is written into `n`.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut n as *mut u32).cast(),
                &mut size,
            )
        };
        (status == 0).then_some(n)
    }

    pub fn adapters() -> Vec<(String, String)> {
        (0..64)
            .filter_map(|i| {
                let key = format!(r"{DISPLAY}\{i:04}");
                let name = text(&key, "DriverDesc")?;
                Some((name, text(&key, "DriverVersion").unwrap_or_default()))
            })
            .collect()
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::CStr;

    pub fn sysctl(name: &CStr) -> Option<String> {
        let mut buf = [0u8; 256];
        let mut len = buf.len();
        // SAFETY: `name` is NUL-terminated; `buf` holds the `len` bytes the value may fill.
        let status = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 {
            return None;
        }
        let text = String::from_utf8_lossy(&buf[..len.min(buf.len())]);
        Some(text.trim_end_matches('\0').to_string())
    }
}
