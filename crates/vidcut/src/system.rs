//! What the computer is: its system's version and its GPUs with their
//! drivers, as the system lists them. `doctor` shows them, and the encoder
//! cache's key holds them, so a new driver tests the encoders again.

use serde_json::{Value, json};

/// The system's version: the kernel's release on Linux, the build on
/// Windows (with its update revision).
pub fn version() -> String {
    #[cfg(windows)]
    {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let build = windows::text(key, "CurrentBuild").unwrap_or_default();
        let revision = windows::number(key, "UBR").unwrap_or(0);
        format!("windows {build}.{revision}")
    }
    #[cfg(not(windows))]
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The GPUs: on Linux the render nodes and their kernel drivers, with
/// NVIDIA's driver version when its module is loaded; on Windows the
/// display adapters and their drivers' versions.
pub fn gpus() -> Vec<Value> {
    #[cfg(windows)]
    {
        windows::adapters()
            .into_iter()
            .map(|(name, driver)| json!({ "name": name, "driver": driver }))
            .collect()
    }
    #[cfg(not(windows))]
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

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };

    /// The display adapters' class: one numbered key for each adapter
    /// installed, with its name and driver.
    const DISPLAY: &str =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A text value of `HKEY_LOCAL_MACHINE\key`.
    pub fn text(key: &str, value: &str) -> Option<String> {
        let (key, value) = (wide(key), wide(value));
        let mut buf = [0u16; 512];
        let mut size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: NUL-terminated names; the value is written into `buf`,
        // `size` bytes at most.
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

    /// A number value of `HKEY_LOCAL_MACHINE\key`.
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

    /// The display adapters installed: each one's name and driver version.
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
