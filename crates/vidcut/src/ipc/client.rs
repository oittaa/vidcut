//! The CLI side of the socket.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use serde_json::Value;
use vidcut_core::{Envelope, Error, ErrorCode, Response, Result};

/// Sends one request to the GUI listening on `socket` and waits for the answer.
/// Progress notifications go to `progress`. `project`, if given, must be the
/// project the GUI has open.
///
/// Setting `cancel` closes the connection: the wait ends with `CANCELLED`,
/// and the GUI stops a foreground export once it can't report progress.
pub fn call(
    socket: &Path,
    envelope: &Envelope,
    project: Option<&Path>,
    progress: &mut dyn FnMut(&Value),
    cancel: Option<&AtomicBool>,
) -> Result<Response> {
    let mut stream = UnixStream::connect(socket).map_err(|e| {
        Error::no_instance(format!(
            "can't reach the vidcut GUI at {}: {e}",
            socket.display()
        ))
    })?;
    let mut line = super::request_json(1, envelope, project)?.to_string();
    line.push('\n');
    let lost = |e: std::io::Error| Error::no_instance(format!("lost the vidcut GUI: {e}"));
    if let Err(e) = stream
        .write_all(line.as_bytes())
        .and_then(|()| stream.flush())
    {
        // The GUI may have turned the connection away, or refused a line too
        // long, and closed it before reading the rest: the write fails, and
        // its answer is still there to read.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        return match read_answer(BufReader::new(stream), progress) {
            Err(answer) if answer.code == ErrorCode::NoInstance => Err(lost(e)),
            answer => answer,
        };
    }
    let watched = match cancel {
        Some(c) => Some((c, stream.try_clone().map_err(lost)?)),
        None => None,
    };
    // Closed when the answer is in, which wakes the watcher at once.
    let (answered, waiting) = channel::<()>();
    std::thread::scope(|scope| {
        if let Some((cancel, s)) = &watched {
            scope.spawn(move || {
                while let Err(RecvTimeoutError::Timeout) =
                    waiting.recv_timeout(Duration::from_millis(20))
                {
                    if cancel.load(Ordering::Relaxed) {
                        let _ = s.shutdown(Shutdown::Both);
                        return;
                    }
                }
            });
        }
        let answer = read_answer(BufReader::new(stream), progress);
        drop(answered);
        match answer {
            Err(_) if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) => {
                Err(Error::new(ErrorCode::Cancelled, "cancelled"))
            }
            answer => answer,
        }
    })
}

/// Reads progress notifications until the answer.
fn read_answer(
    mut reader: BufReader<UnixStream>,
    progress: &mut dyn FnMut(&Value),
) -> Result<Response> {
    let lost = |e: std::io::Error| Error::no_instance(format!("lost the vidcut GUI: {e}"));
    loop {
        let mut buf = String::new();
        if reader.read_line(&mut buf).map_err(lost)? == 0 {
            return Err(Error::no_instance(
                "the vidcut GUI closed the connection without answering",
            ));
        }
        let v: Value = serde_json::from_str(&buf)
            .map_err(|e| Error::internal(format!("bad reply from the vidcut GUI: {e}")))?;
        // One request per connection: an error answers it, even without an
        // id (the GUI couldn't read the request, or turned the connection
        // away).
        if let Some(e) = v.get("error") {
            return Err(super::error_from_json(e));
        }
        if v.get("id").is_none() || v["id"].is_null() {
            if v["method"] == "progress" {
                progress(&v["params"]);
            }
            continue;
        }
        return serde_json::from_value(v["result"].clone())
            .map_err(|e| Error::internal(format!("bad reply from the vidcut GUI: {e}")));
    }
}
