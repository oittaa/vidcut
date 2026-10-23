pub use imp::{Listener, Stream, exists};

#[cfg(any(windows, test))]
const PIPE_PREFIX: &str = r"\\.\pipe\";

#[cfg(any(windows, test))]
fn has_pipe_prefix(text: &str) -> bool {
    text.get(..PIPE_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(PIPE_PREFIX))
}

#[cfg(unix)]
mod imp {
    use std::io::{self, Read, Write};
    use std::net::Shutdown;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::time::Duration;

    pub struct Listener(UnixListener);

    impl Listener {
        pub fn bind(path: &Path) -> io::Result<Self> {
            let _ = std::fs::remove_file(path);
            UnixListener::bind(path).map(Self)
        }

        pub fn accept(&self) -> io::Result<Stream> {
            self.0.accept().map(|(s, _)| Stream(s))
        }
    }

    pub struct Stream(UnixStream);

    impl Stream {
        pub fn connect(path: &Path) -> io::Result<Self> {
            UnixStream::connect(path).map(Self)
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            self.0.try_clone().map(Self)
        }

        pub fn shutdown(&self) -> io::Result<()> {
            self.0.shutdown(Shutdown::Both)
        }

        pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.0.set_read_timeout(timeout)
        }

        pub fn peer_gone(&self) -> bool {
            use std::os::fd::AsRawFd;
            let mut pfd = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: `pfd` is one `pollfd`, for this stream's open socket.
            let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
            rc > 0 && pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
        }
    }

    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for Stream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    pub fn exists(path: &Path) -> bool {
        path.exists()
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io::{self, Read, Write};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_INVALID_HANDLE, ERROR_IO_PENDING, ERROR_NO_DATA,
        ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, ERROR_SEM_TIMEOUT,
        GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
        WAIT_OBJECT_0, WAIT_TIMEOUT, WIN32_ERROR,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, PeekNamedPipe, WaitNamedPipeW,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject,
    };

    const POLL: Duration = Duration::from_millis(50);

    const BUFFER: u32 = 64 << 10;

    struct Handle(HANDLE);

    // SAFETY: kernel handles may be used, and closed, from any thread.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and still open.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn last_error() -> WIN32_ERROR {
        // SAFETY: reads this thread's last error.
        unsafe { GetLastError() }
    }

    fn error(code: WIN32_ERROR) -> io::Error {
        io::Error::from_raw_os_error(code as i32)
    }

    fn gone(code: WIN32_ERROR) -> bool {
        matches!(
            code,
            ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA | ERROR_INVALID_HANDLE
        )
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn pipe_name(path: &Path) -> Vec<u16> {
        const PREFIX: &str = super::PIPE_PREFIX;
        let text = path.to_string_lossy();
        if super::has_pipe_prefix(&text) {
            return wide(&text);
        }
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let key = absolute.to_string_lossy().to_lowercase();
        let name = vidcut_core::fingerprint::short_digest(key.as_bytes());
        wide(&format!(r"{PREFIX}vidcut-{name}"))
    }

    fn event() -> io::Result<Handle> {
        // SAFETY: plain event creation; null attributes and name.
        let h = unsafe { CreateEventW(null(), 1, 0, null()) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Handle(h))
    }

    struct Security(*mut c_void);

    // SAFETY: the descriptor is only read once made.
    unsafe impl Send for Security {}
    unsafe impl Sync for Security {}

    impl Security {
        fn this_user() -> io::Result<Self> {
            let sid = user_sid()?;
            let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
            let mut descriptor = null_mut();
            // SAFETY: `sddl` is NUL-terminated; the descriptor out is LocalAlloc'd, freed on drop.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(descriptor))
        }

        fn attributes(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.0,
                bInheritHandle: 0,
            }
        }
    }

    impl Drop for Security {
        fn drop(&mut self) {
            // SAFETY: LocalAlloc'd by the conversion above.
            unsafe { LocalFree(self.0) };
        }
    }

    fn user_sid() -> io::Result<String> {
        // SAFETY: `buf` is the `len` bytes asked, aligned for `TOKEN_USER`; `text` ends in NUL.
        unsafe {
            let mut token = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = Handle(token);
            let mut len = 0u32;
            GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut len);
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            if GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut text = null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            Ok(sid)
        }
    }

    fn instance(name: &[u16], security: &Security, first: bool) -> io::Result<Handle> {
        let attributes = security.attributes();
        let first = if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
        // SAFETY: a NUL-terminated name and attributes that outlive the call.
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | first,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER,
                BUFFER,
                0,
                &attributes,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Handle(h))
    }

    pub struct Listener {
        name: Vec<u16>,
        security: Security,
        next: Mutex<Option<Handle>>,
        event: Handle,
    }

    impl Listener {
        pub fn bind(path: &Path) -> io::Result<Self> {
            let name = pipe_name(path);
            let security = Security::this_user()?;
            let first = instance(&name, &security, true)?;
            Ok(Self {
                name,
                security,
                next: Mutex::new(Some(first)),
                event: event()?,
            })
        }

        pub fn accept(&self) -> io::Result<Stream> {
            loop {
                let waiting = self.next.lock().unwrap_or_else(|e| e.into_inner()).take();
                let pipe = match waiting {
                    Some(p) => p,
                    None => instance(&self.name, &self.security, false)?,
                };
                // SAFETY: `ov` outlives the connect: a pending one is waited for before `ov` goes.
                let connected = unsafe {
                    let mut ov: OVERLAPPED = std::mem::zeroed();
                    ov.hEvent = self.event.0;
                    if ConnectNamedPipe(pipe.0, &mut ov) != 0 {
                        Ok(())
                    } else {
                        match last_error() {
                            ERROR_PIPE_CONNECTED => Ok(()),
                            ERROR_IO_PENDING => {
                                let mut n = 0;
                                if GetOverlappedResult(pipe.0, &ov, &mut n, 1) != 0 {
                                    Ok(())
                                } else {
                                    Err(last_error())
                                }
                            }
                            e => Err(e),
                        }
                    }
                };
                *self.next.lock().unwrap_or_else(|e| e.into_inner()) =
                    instance(&self.name, &self.security, false).ok();
                match connected {
                    Ok(()) => return Stream::new(pipe),
                    Err(e) if gone(e) => continue,
                    Err(e) => return Err(error(e)),
                }
            }
        }
    }

    struct End {
        pipe: Handle,
        down: AtomicBool,
        timeout_ms: AtomicU64,
    }

    pub struct Stream {
        end: Arc<End>,
        event: Handle,
    }

    impl Stream {
        fn new(pipe: Handle) -> io::Result<Self> {
            Ok(Self {
                end: Arc::new(End {
                    pipe,
                    down: AtomicBool::new(false),
                    timeout_ms: AtomicU64::new(0),
                }),
                event: event()?,
            })
        }

        pub fn connect(path: &Path) -> io::Result<Self> {
            let name = pipe_name(path);
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                // SAFETY: `name` is NUL-terminated; null attributes and template are allowed.
                let h = unsafe {
                    CreateFileW(
                        name.as_ptr(),
                        GENERIC_READ | GENERIC_WRITE,
                        0,
                        null(),
                        OPEN_EXISTING,
                        FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                        null_mut(),
                    )
                };
                if h != INVALID_HANDLE_VALUE {
                    return Self::new(Handle(h));
                }
                let e = last_error();
                let left = until.saturating_duration_since(Instant::now());
                if e != ERROR_PIPE_BUSY || left.is_zero() {
                    return Err(error(e));
                }
                // SAFETY: a NUL-terminated name.
                unsafe { WaitNamedPipeW(name.as_ptr(), left.as_millis().max(1) as u32) };
            }
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            Ok(Self {
                end: self.end.clone(),
                event: event()?,
            })
        }

        pub fn shutdown(&self) -> io::Result<()> {
            self.end.down.store(true, Ordering::Release);
            // SAFETY: an open pipe; `transfer` waits out each cancelled call before its buffers go.
            unsafe { CancelIoEx(self.end.pipe.0, null()) };
            Ok(())
        }

        pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            let ms = timeout.map_or(0, |t| t.as_millis().clamp(1, u128::from(u64::MAX)) as u64);
            self.end.timeout_ms.store(ms, Ordering::Release);
            Ok(())
        }

        pub fn peer_gone(&self) -> bool {
            let mut available = 0u32;
            // SAFETY: peeks at nothing, only asks how much is there.
            let ok = unsafe {
                PeekNamedPipe(
                    self.end.pipe.0,
                    null_mut(),
                    0,
                    null_mut(),
                    &mut available,
                    null_mut(),
                )
            };
            self.end.down.load(Ordering::Acquire) || (ok == 0 && gone(last_error()))
        }

        fn transfer(
            &self,
            start: impl FnOnce(&mut OVERLAPPED) -> i32,
            deadline: Option<Instant>,
        ) -> io::Result<usize> {
            let pipe = self.end.pipe.0;
            // SAFETY: every way out waits for the call to end, so `ov` and the buffer outlive it.
            unsafe {
                let mut ov: OVERLAPPED = std::mem::zeroed();
                ov.hEvent = self.event.0;
                if start(&mut ov) == 0 {
                    let e = last_error();
                    if e != ERROR_IO_PENDING {
                        return Err(error(e));
                    }
                }
                loop {
                    let slice = deadline.map_or(POLL, |d| {
                        d.saturating_duration_since(Instant::now()).min(POLL)
                    });
                    match WaitForSingleObject(self.event.0, slice.as_millis() as u32) {
                        WAIT_OBJECT_0 => break,
                        WAIT_TIMEOUT => {
                            let down = self.end.down.load(Ordering::Acquire);
                            let late = deadline.is_some_and(|d| Instant::now() >= d);
                            if down || late {
                                CancelIoEx(pipe, &ov);
                                let mut n = 0;
                                if GetOverlappedResult(pipe, &ov, &mut n, 1) != 0 && n > 0 {
                                    return Ok(n as usize);
                                }
                                return Err(if down {
                                    io::ErrorKind::BrokenPipe.into()
                                } else {
                                    io::ErrorKind::TimedOut.into()
                                });
                            }
                        }
                        _ => {
                            let e = io::Error::last_os_error();
                            CancelIoEx(pipe, &ov);
                            let mut n = 0;
                            GetOverlappedResult(pipe, &ov, &mut n, 1);
                            return Err(e);
                        }
                    }
                }
                let mut n = 0;
                if GetOverlappedResult(pipe, &ov, &mut n, 0) == 0 {
                    return Err(error(last_error()));
                }
                Ok(n as usize)
            }
        }
    }

    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.end.down.load(Ordering::Acquire) || buf.is_empty() {
                return Ok(0);
            }
            let deadline = match self.end.timeout_ms.load(Ordering::Acquire) {
                0 => None,
                ms => Some(Instant::now() + Duration::from_millis(ms)),
            };
            let pipe = self.end.pipe.0;
            let len = buf.len().min(u32::MAX as usize) as u32;
            let ptr = buf.as_mut_ptr();
            let read = self.transfer(
                // SAFETY: `buf` outlives the call (`transfer` waits for it).
                |ov| unsafe { ReadFile(pipe, ptr, len, null_mut(), ov) },
                deadline,
            );
            match read {
                Err(_) if self.end.down.load(Ordering::Acquire) => Ok(0),
                Err(e) if e.raw_os_error().is_some_and(|c| gone(c as u32)) => Ok(0),
                r => r,
            }
        }
    }

    impl Write for Stream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.end.down.load(Ordering::Acquire) {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let pipe = self.end.pipe.0;
            let len = buf.len().min(u32::MAX as usize) as u32;
            let ptr = buf.as_ptr();
            let written = self.transfer(
                // SAFETY: `buf` outlives the call (`transfer` waits for it).
                |ov| unsafe { WriteFile(pipe, ptr, len, null_mut(), ov) },
                None,
            );
            match written {
                Err(e) if e.raw_os_error().is_some_and(|c| gone(c as u32)) => {
                    Err(io::ErrorKind::BrokenPipe.into())
                }
                w => w,
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub fn exists(path: &Path) -> bool {
        let name = pipe_name(path);
        // SAFETY: `name` is NUL-terminated.
        unsafe { WaitNamedPipeW(name.as_ptr(), 1) != 0 || GetLastError() == ERROR_SEM_TIMEOUT }
    }
}

#[cfg(test)]
mod tests {
    use super::has_pipe_prefix;

    #[test]
    fn a_multibyte_character_does_not_panic_in_the_pipe_prefix() {
        let text = "C:\\abc\u{1F600}";
        let n = r"\\.\pipe\".len();
        assert!(
            std::panic::catch_unwind(|| {
                let _ = &text[..n];
            })
            .is_err()
        );
        assert!(!has_pipe_prefix(text));
    }

    #[test]
    fn a_pipe_prefix_matches_in_either_case() {
        assert!(has_pipe_prefix(r"\\.\pipe\vidcut-1"));
        assert!(has_pipe_prefix(r"\\.\PIPE\vidcut-1"));
        assert!(!has_pipe_prefix(r"C:\vidcut"));
    }
}
