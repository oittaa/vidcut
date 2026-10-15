//! The computer's own look: light or dark, and its accent color. Native
//! themes follow it (deviation X22): Windows' settings, macOS's appearance
//! and accent color, and the desktop portal's on Linux. [`watch`] says when
//! it may have changed.

use std::sync::Arc;

use eframe::egui::Color32;

/// Light or dark, and the accent color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub light: bool,
    pub accent: Accent,
}

/// An accent color with a lighter and a darker shade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Accent {
    pub base: Color32,
    /// For text and lines on dark backgrounds.
    pub light: Color32,
    /// For fills under light text.
    pub dark: Color32,
}

impl Accent {
    /// `base` with its shades mixed toward white and black.
    pub fn from_base(base: Color32) -> Self {
        Self {
            base,
            light: mix(base, Color32::WHITE, 0.45),
            dark: mix(base, Color32::BLACK, 0.15),
        }
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Dark, in Windows' default blue: where the computer says nothing.
pub const DEFAULT: Look = Look {
    light: false,
    accent: Accent {
        base: Color32::from_rgb(0, 120, 212),
        light: Color32::from_rgb(76, 194, 255),
        dark: Color32::from_rgb(0, 103, 192),
    },
};

/// The look now. What the computer doesn't say comes from [`DEFAULT`].
pub fn current() -> Look {
    let mut look = DEFAULT;
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    {
        if let Some(light) = os::light() {
            look.light = light;
        }
        if let Some(accent) = os::accent() {
            look.accent = accent;
        }
    }
    look
}

/// Calls `changed` whenever the look may have changed, from threads of its
/// own, for as long as the app runs. Once is enough.
pub fn watch(changed: Arc<dyn Fn() + Send + Sync>) {
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    os::watch(changed);
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let _ = changed;
}

#[cfg(windows)]
mod os {
    use std::sync::Arc;

    use eframe::egui::Color32;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, RRF_RT_REG_BINARY,
        RRF_RT_REG_DWORD, RegCloseKey, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW,
    };

    use super::Accent;

    const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
    const ACCENT: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// `HKEY_CURRENT_USER\key`'s `value`, `buf.len()` bytes at most, of the
    /// kind `flags` names. The number of bytes read.
    fn read(key: &str, value: &str, flags: u32, buf: &mut [u8]) -> Option<usize> {
        let (key, value) = (wide(key), wide(value));
        let mut size = buf.len() as u32;
        // SAFETY: NUL-terminated names; the value is written into `buf`,
        // `size` bytes at most.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                flags,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        (status == 0).then_some(size as usize)
    }

    /// Apps in light mode (Settings > Personalization > Colors).
    pub fn light() -> Option<bool> {
        let mut n = [0u8; 4];
        read(PERSONALIZE, "AppsUseLightTheme", RRF_RT_REG_DWORD, &mut n)?;
        Some(u32::from_le_bytes(n) != 0)
    }

    /// Windows' own shades of the accent color: lighter ones, the accent,
    /// then darker ones, four bytes (RGBA) each.
    pub fn accent() -> Option<Accent> {
        let mut p = [0u8; 32];
        let n = read(ACCENT, "AccentPalette", RRF_RT_REG_BINARY, &mut p)?;
        if n < 20 {
            return None;
        }
        let shade = |i: usize| Color32::from_rgb(p[i * 4], p[i * 4 + 1], p[i * 4 + 2]);
        Some(Accent {
            base: shade(3),
            light: shade(1),
            dark: shade(4),
        })
    }

    /// A thread per key, waiting for its values to change.
    pub fn watch(changed: Arc<dyn Fn() + Send + Sync>) {
        for key in [PERSONALIZE, ACCENT] {
            let changed = changed.clone();
            let _ = crate::threads::spawn(
                "vidcut-look",
                crate::threads::Work::Interactive,
                move || {
                    let name = wide(key);
                    let mut hkey: HKEY = std::ptr::null_mut();
                    // SAFETY: a NUL-terminated name; the key is written into
                    // `hkey`.
                    let status = unsafe {
                        RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, KEY_NOTIFY, &mut hkey)
                    };
                    if status != 0 {
                        return;
                    }
                    // SAFETY: an open key; without an event the call waits
                    // until a value of the key changes.
                    while unsafe {
                        RegNotifyChangeKeyValue(
                            hkey,
                            0,
                            REG_NOTIFY_CHANGE_LAST_SET,
                            std::ptr::null_mut(),
                            0,
                        )
                    } == 0
                    {
                        changed();
                    }
                    // SAFETY: the key opened above.
                    unsafe { RegCloseKey(hkey) };
                },
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::sync::Arc;
    use std::time::Duration;

    use eframe::egui::Color32;
    use objc2_app_kit::{NSColor, NSColorSpace};
    use objc2_foundation::{NSUserDefaults, ns_string};

    use super::Accent;

    /// The appearance (System Settings > Appearance): `AppleInterfaceStyle`
    /// is `Dark` while dark.
    pub fn light() -> Option<bool> {
        let style =
            NSUserDefaults::standardUserDefaults().stringForKey(ns_string!("AppleInterfaceStyle"));
        Some(style.is_none_or(|s| s.to_string() != "Dark"))
    }

    /// The accent color in sRGB.
    pub fn accent() -> Option<Accent> {
        let c =
            NSColor::controlAccentColor().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
        let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some(Accent::from_base(Color32::from_rgb(
            byte(c.redComponent()),
            byte(c.greenComponent()),
            byte(c.blueComponent()),
        )))
    }

    /// Reads the look every two seconds: two settings, no window needed.
    pub fn watch(changed: Arc<dyn Fn() + Send + Sync>) {
        let _ = crate::threads::spawn(
            "vidcut-look",
            crate::threads::Work::Interactive,
            move || {
                let mut last = super::current();
                loop {
                    std::thread::sleep(Duration::from_secs(2));
                    let now = super::current();
                    if now != last {
                        last = now;
                        changed();
                    }
                }
            },
        );
    }
}

#[cfg(target_os = "linux")]
mod os {
    use std::sync::{Arc, OnceLock};

    use eframe::egui::Color32;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedValue, Value};

    use super::Accent;

    const APPEARANCE: &str = "org.freedesktop.appearance";

    /// The desktop portal's settings, on the session bus.
    fn settings() -> Option<Proxy<'static>> {
        static BUS: OnceLock<Option<Connection>> = OnceLock::new();
        let bus = BUS.get_or_init(|| Connection::session().ok()).as_ref()?;
        Proxy::new(
            bus,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )
        .ok()
    }

    /// An appearance setting: `ReadOne`, or the older `Read`, which wraps
    /// it once more.
    fn read(key: &str) -> Option<OwnedValue> {
        let settings = settings()?;
        if let Ok(v) = settings.call::<_, _, OwnedValue>("ReadOne", &(APPEARANCE, key)) {
            return Some(v);
        }
        let v: OwnedValue = settings.call("Read", &(APPEARANCE, key)).ok()?;
        match &*v {
            Value::Value(inner) => OwnedValue::try_from(inner.as_ref().try_clone().ok()?).ok(),
            _ => Some(v),
        }
    }

    /// `color-scheme`: 1 prefers dark, 2 light, 0 says nothing.
    pub fn light() -> Option<bool> {
        match u32::try_from(read("color-scheme")?).ok()? {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
    }

    /// `accent-color`: red, green and blue from 0 to 1, outside that range
    /// when none is set.
    pub fn accent() -> Option<Accent> {
        let (r, g, b) = <(f64, f64, f64)>::try_from(read("accent-color")?).ok()?;
        if ![r, g, b].iter().all(|c| (0.0..=1.0).contains(c)) {
            return None;
        }
        let byte = |v: f64| (v * 255.0).round() as u8;
        Some(Accent::from_base(Color32::from_rgb(
            byte(r),
            byte(g),
            byte(b),
        )))
    }

    /// Waits on the portal's `SettingChanged` signal.
    pub fn watch(changed: Arc<dyn Fn() + Send + Sync>) {
        let _ = crate::threads::spawn(
            "vidcut-look",
            crate::threads::Work::Interactive,
            move || {
                let Some(settings) = settings() else {
                    return;
                };
                let Ok(signals) = settings.receive_signal("SettingChanged") else {
                    return;
                };
                for message in signals {
                    let namespace = message
                        .body()
                        .deserialize::<(String, String, OwnedValue)>()
                        .map(|(namespace, _, _)| namespace);
                    if namespace.is_ok_and(|n| n == APPEARANCE) {
                        changed();
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accents_shades_mix_toward_white_and_black() {
        let a = Accent::from_base(Color32::from_rgb(100, 0, 200));
        assert_eq!(
            (a.light, a.dark),
            (
                Color32::from_rgb(170, 115, 225),
                Color32::from_rgb(85, 0, 170)
            )
        );
    }
}
