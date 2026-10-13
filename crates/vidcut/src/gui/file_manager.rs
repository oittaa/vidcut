//! Show in Folder: the file manager showing a file, selected in its
//! folder, or a folder.

use std::path::Path;

/// Shows `path` in the file manager: a folder opened, a file selected in
/// its folder (Explorer, Finder). On Linux the file's folder is opened:
/// xdg-open selects nothing.
pub fn show(path: &Path) {
    let path = path.to_path_buf();
    // On a thread of its own: the shell's calls wait for Explorer, and a
    // program started is waited for, so it doesn't stay a zombie.
    let _ = crate::threads::spawn(
        "vidcut-show-in-folder",
        crate::threads::Work::Interactive,
        move || show_now(&path),
    );
}

/// Through the shell's own calls, not a program vidcut starts: that would
/// inherit every inheritable handle vidcut has open, x264's two-pass
/// statistics files among them, and x264 can't rename a file another
/// process holds (docs/gotchas.md). What the shell starts inherits none.
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
    // SAFETY: COM for this thread, undone below unless it failed;
    // NUL-terminated paths in; the item list made here is freed here.
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
        // A folder is opened, and so is the folder of a file that can't be
        // selected (gone since).
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

/// Through `open` (macOS) or xdg-open. The program inherits the
/// descriptors vidcut has open without close-on-exec, x264's statistics
/// files among them: harmless here, where a file another process has open
/// can still be renamed.
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

#[cfg(all(test, windows))]
mod tests {
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::null;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_MORE_DATA, ERROR_SUCCESS, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::System::RestartManager::{
        CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources,
        RmStartSession,
    };
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, TerminateProcess};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
        ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    // The C runtime's, which x264 opens its statistics files with.
    unsafe extern "C" {
        fn _wfopen(name: *const u16, mode: *const u16) -> *mut c_void;
        fn fclose(file: *mut c_void) -> i32;
    }

    fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
        text.as_ref().encode_wide().chain([0]).collect()
    }

    /// The processes that have `path` open, as Restart Manager finds them.
    fn holders(path: &Path) -> Vec<u32> {
        let name = wide(path);
        let mut session = 0;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        let mut found: Vec<RM_PROCESS_INFO> = Vec::new();
        // SAFETY: a session of this test's own, ended below; each buffer
        // goes with its length.
        unsafe {
            assert_eq!(
                RmStartSession(&mut session, 0, key.as_mut_ptr()),
                ERROR_SUCCESS
            );
            let names = [name.as_ptr()];
            assert_eq!(
                RmRegisterResources(session, 1, names.as_ptr(), 0, null(), 0, null()),
                ERROR_SUCCESS
            );
            let mut needed = 0;
            loop {
                found.resize(needed as usize, std::mem::zeroed());
                let mut count = found.len() as u32;
                let mut reasons = 0;
                let r = RmGetList(
                    session,
                    &mut needed,
                    &mut count,
                    found.as_mut_ptr(),
                    &mut reasons,
                );
                if r != ERROR_MORE_DATA {
                    assert_eq!(r, ERROR_SUCCESS);
                    found.truncate(count as usize);
                    break;
                }
            }
            RmEndSession(session);
        }
        found.iter().map(|p| p.Process.dwProcessId).collect()
    }

    /// x264 opens its two-pass statistics files with the C runtime, whose
    /// handles are inheritable: a program std's `Command` starts holds them
    /// while it runs, and x264 can't rename them. A program the shell
    /// starts for Show in Folder (the user's own file manager, or Explorer
    /// when none runs) holds none of them.
    #[test]
    fn a_program_the_shell_starts_holds_none_of_our_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.temp");
        // SAFETY: NUL-terminated strings; the file is closed below.
        let file = unsafe { _wfopen(wide(&path).as_ptr(), wide("wb").as_ptr()) };
        assert!(!file.is_null());
        // Hidden, running until it is stopped below.
        let ping = Path::new(&std::env::var_os("SystemRoot").unwrap()).join(r"System32\PING.EXE");
        let (program, parameters) = (wide(&ping), wide("-n 60 127.0.0.1"));
        let mut run = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpFile: program.as_ptr(),
            lpParameters: parameters.as_ptr(),
            nShow: SW_HIDE,
            // SAFETY: zero is a valid value for each of the other fields.
            ..unsafe { std::mem::zeroed() }
        };
        // SAFETY: COM for this thread, undone below; the strings outlive
        // the call.
        let started = unsafe {
            CoInitializeEx(
                null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            );
            ShellExecuteExW(&mut run) != 0
        };
        assert!(started, "{}", std::io::Error::last_os_error());
        let held = holders(&path);
        let mut code = 0;
        // SAFETY: the process handle asked for above, closed here, and the
        // file opened above.
        unsafe {
            GetExitCodeProcess(run.hProcess, &mut code);
            TerminateProcess(run.hProcess, 1);
            CloseHandle(run.hProcess);
            fclose(file);
            CoUninitialize();
        }
        assert_eq!(code, STILL_ACTIVE as u32, "ping ended before the check");
        assert_eq!(held, [std::process::id()], "the processes holding the file");
    }
}
