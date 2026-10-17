use std::path::Path;

pub fn show(path: &Path) {
    let path = path.to_path_buf();
    let _ = crate::threads::spawn(
        "vidcut-show-in-folder",
        crate::threads::Work::Interactive,
        move || show_now(&path),
    );
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
    let file = !path.is_dir();
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        if file {
            c.arg("-R");
        }
        c.arg(path);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(if file {
            path.parent().unwrap_or(path)
        } else {
            path
        });
        c
    };
    let _ = command.status();
}
