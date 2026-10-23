use std::path::Path;
#[cfg(any(not(windows), test))]
use std::path::PathBuf;

pub fn show(path: &Path) {
    let path = path.to_path_buf();
    let _ = crate::threads::spawn(
        "vidcut-show-in-folder",
        crate::threads::Work::Interactive,
        move || show_now(&path),
    );
}

pub fn open(path: &Path) {
    let path = path.to_path_buf();
    let _ = crate::threads::spawn(
        "vidcut-open-file",
        crate::threads::Work::Interactive,
        move || open_now(&path),
    );
}

#[cfg(windows)]
fn open_now(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if runs_when_opened(&path) {
        show_now(&path);
        return;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: a NUL-terminated path and verb; the other arguments may be null.
    unsafe {
        ShellExecuteW(
            null_mut(),
            windows_sys::w!("open"),
            wide.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(not(windows))]
fn open_now(path: &Path) {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    let path = for_opener(path);
    let _ = std::process::Command::new(opener).arg(&path).status();
}

#[cfg(windows)]
fn show_now(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::{
        ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems, ShellExecuteW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = path.is_dir();
    // SAFETY: COM is undone only if it started; paths are NUL-terminated; `item` is ILFree'd once.
    unsafe {
        let com = CoInitializeEx(
            null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
        let mut selected = false;
        if !dir {
            let item = ILCreateFromPathW(wide(&path).as_ptr());
            if !item.is_null() {
                selected = SHOpenFolderAndSelectItems(item, 0, null(), 0) >= 0;
                ILFree(item);
            }
        }
        if !selected {
            let folder = if dir {
                path.as_path()
            } else {
                path.parent().unwrap_or(path.as_path())
            };
            ShellExecuteW(
                null_mut(),
                windows_sys::w!("open"),
                wide(folder).as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            );
        }
        if com >= 0 {
            CoUninitialize();
        }
    }
}

#[cfg(not(windows))]
fn show_now(path: &Path) {
    use std::process::Command;
    let path = for_opener(path);
    let file = !path.is_dir();
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        if file {
            c.arg("-R");
        }
        c.arg(&path);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(if file {
            path.parent().unwrap_or(&path)
        } else {
            path.as_path()
        });
        c
    };
    let _ = command.status();
}

#[cfg(any(not(windows), test))]
fn for_opener(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if path.as_os_str().to_string_lossy().starts_with('-') {
        Path::new(".").join(path)
    } else {
        path
    }
}

#[cfg(any(windows, test))]
fn runs_when_opened(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "exe"
            | "com"
            | "bat"
            | "cmd"
            | "scr"
            | "pif"
            | "msi"
            | "msp"
            | "hta"
            | "cpl"
            | "js"
            | "jse"
            | "vbs"
            | "vbe"
            | "wsf"
            | "wsh"
            | "ps1"
            | "psc1"
            | "psm1"
            | "msc"
            | "lnk"
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::{for_opener, runs_when_opened};

    #[test]
    fn a_relative_dash_path_is_absolute() {
        let got = for_opener(Path::new("-clip.mp4"));
        assert!(got.is_absolute());
        assert_eq!(got.file_name(), Some(OsStr::new("-clip.mp4")));
        assert!(!got.to_string_lossy().starts_with('-'));
    }

    #[test]
    fn an_executable_name_would_run() {
        assert!(runs_when_opened(Path::new("export.EXE")));
    }

    #[test]
    fn a_movie_name_would_not_run() {
        assert!(!runs_when_opened(Path::new("export.mp4")));
    }
}
