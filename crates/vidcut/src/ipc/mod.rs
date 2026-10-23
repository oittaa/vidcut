pub mod client;
pub mod registry;
pub mod server;
pub mod transport;

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vidcut_core::{Envelope, Error, ErrorCode, Request, Result};

pub use registry::Instance;

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Headless,
    Instance(Instance),
}

#[derive(Debug, Clone, Default)]
pub struct RouteOptions {
    pub headless: bool,
    pub instance: Option<String>,
}

pub fn run(
    project: Option<&Path>,
    options: &RouteOptions,
    envelope: Envelope,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    progress: &mut dyn FnMut(&Value),
) -> Result<vidcut_core::Response> {
    match route(project, options, &envelope.request)? {
        Route::Headless => crate::session::run_with(
            project,
            envelope,
            &mut crate::session::Hooks {
                cancel,
                progress: &mut |i, n, path, p| {
                    progress(&json!({
                        "output": i, "outputs": n, "path": path,
                        "frames": p.frames, "total": p.total,
                        "pass": p.pass, "passes": p.passes, "measuring": p.measuring,
                        "stage": p.stage, "elapsed": p.elapsed.as_secs_f64(),
                    }))
                },
            },
        ),
        Route::Instance(instance) => {
            let project = project.and_then(|p| vidcut_core::dirs::canonical(p).ok());
            client::call(
                &instance.socket,
                &envelope,
                project.as_deref(),
                progress,
                Some(&**cancel),
            )
        }
    }
}

pub fn route(project: Option<&Path>, options: &RouteOptions, request: &Request) -> Result<Route> {
    if request.is_projectless() {
        return Ok(Route::Headless);
    }
    if options.headless {
        return Ok(Route::Headless);
    }
    if let Some(which) = &options.instance {
        return find_explicit(which).map(Route::Instance);
    }
    let live = request.is_live();
    let instances = registry::list();
    if let Some(project) = project {
        let want = vidcut_core::dirs::canonical(project).ok();
        if let Some(i) = instances
            .iter()
            .find(|i| i.project.is_some() && i.project.as_deref() == want.as_deref())
        {
            return Ok(Route::Instance(i.clone()));
        }
        if live {
            return Err(Error::no_instance(format!(
                "no running vidcut GUI has {} open; start one with `vidcut -p {}`",
                project.display(),
                project.display()
            )));
        }
        return Ok(Route::Headless);
    }
    let candidates: Vec<&Instance> = if live {
        instances.iter().collect()
    } else {
        instances.iter().filter(|i| i.project.is_some()).collect()
    };
    match candidates.as_slice() {
        [one] => Ok(Route::Instance((*one).clone())),
        [] if live => Err(Error::no_instance(format!(
            "{} needs a running vidcut GUI; start one with `vidcut` or `vidcut -p FILE`",
            request.method()
        ))),
        [] => Ok(Route::Headless),
        many => Err(Error::usage(format!(
            "{} vidcut GUIs are running ({}); pick one with -p FILE or --instance PID",
            many.len(),
            many.iter()
                .map(|i| i.pid.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn find_explicit(which: &str) -> Result<Instance> {
    if let Ok(pid) = which.parse::<u32>() {
        return registry::list()
            .into_iter()
            .find(|i| i.pid == pid)
            .ok_or_else(|| Error::no_instance(format!("no running vidcut GUI with pid {pid}")));
    }
    let socket = PathBuf::from(which);
    Ok(registry::list()
        .into_iter()
        .find(|i| i.socket == socket)
        .unwrap_or(Instance {
            pid: 0,
            socket,
            project: None,
            version: String::new(),
        }))
}

pub fn request_json(id: u64, envelope: &Envelope, project: Option<&Path>) -> Result<Value> {
    let Value::Object(mut m) =
        serde_json::to_value(&envelope.request).map_err(|e| Error::internal(e.to_string()))?
    else {
        return Err(Error::internal("request did not serialize to an object"));
    };
    let method = m.remove("method").unwrap_or(Value::Null);
    let mut params = m.remove("params").unwrap_or_else(|| json!({}));
    if let Some(p) = params.as_object_mut() {
        if let Some(rev) = envelope.if_rev {
            p.insert("if_rev".into(), json!(rev));
        }
        if envelope.dry_run {
            p.insert("dry_run".into(), json!(true));
        }
    }
    let mut v = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    if let Some(client) = &envelope.client {
        v["client"] = json!(client);
    }
    if let Some(project) = project {
        v["project"] = json!(project);
    }
    Ok(v)
}

pub fn error_json(e: &Error) -> Value {
    json!({ "code": e.code.exit_code(), "message": e.message, "data": { "code": e.code } })
}

pub fn error_from_json(v: &Value) -> Error {
    let code = v
        .get("data")
        .and_then(|d| d.get("code"))
        .and_then(|c| serde_json::from_value::<ErrorCode>(c.clone()).ok())
        .unwrap_or(ErrorCode::Internal);
    let message = v
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error from the vidcut GUI")
        .to_string();
    Error::new(code, message)
}

pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("vidcut"),
        #[cfg(unix)]
        None => std::env::temp_dir().join(format!("vidcut-{}", unsafe { libc::getuid() })),
        #[cfg(not(unix))]
        None => std::env::temp_dir().join("vidcut"),
    }
}

pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        private_dir_unix(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

pub(crate) fn read_capped_line(
    reader: &mut impl std::io::BufRead,
    max: u64,
) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = reader
        .by_ref()
        .take(max.saturating_add(1))
        .read_until(b'\n', &mut buf)?;
    if n == 0 && buf.is_empty() {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }
    if buf.len() as u64 > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("a line is limited to {max} bytes"),
        ));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(unix)]
fn private_dir_unix(dir: &Path) -> std::io::Result<()> {
    use std::io::ErrorKind;
    use std::os::unix::fs::DirBuilderExt;
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    if let Ok(meta) = std::fs::symlink_metadata(&dir)
        && !real_directory(&meta)
    {
        return Err(not_private(&dir));
    }
    let mut missing = Vec::new();
    let mut cursor = dir.clone();
    loop {
        match std::fs::symlink_metadata(&cursor) {
            Ok(_) => break,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let Some(parent) = cursor.parent().map(Path::to_path_buf) else {
                    return Err(not_private(&cursor));
                };
                if parent.as_os_str().is_empty() || parent == cursor {
                    return Err(not_private(&cursor));
                }
                missing.push(cursor);
                cursor = parent;
            }
            Err(e) => return Err(e),
        }
    }
    if cursor != dir {
        ready_parent(&cursor)?;
    } else if let Some(parent) = dir.parent()
        && !parent.as_os_str().is_empty()
        && std::fs::symlink_metadata(parent).is_ok()
    {
        ready_parent(parent)?;
    }
    for path in missing.iter().rev() {
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        tighten(path)?;
    }
    if missing.is_empty() {
        tighten(&dir)?;
    }
    Ok(())
}

#[cfg(unix)]
fn ready_parent(parent: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(parent)?;
    if !real_directory(&meta) {
        return Err(not_private(parent));
    }
    let uid = current_uid();
    if meta.uid() != uid && !sticky(&meta) {
        return Err(not_private(parent));
    }
    if meta.uid() == uid && vidcut_dir_name(parent) {
        tighten(parent)?;
    }
    let meta = std::fs::symlink_metadata(parent)?;
    if strangers_can_replace(&meta) {
        return Err(not_private(parent));
    }
    Ok(())
}

#[cfg(unix)]
fn tighten(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let uid = current_uid();
    let meta = std::fs::symlink_metadata(path)?;
    if meta.uid() != uid || !real_directory(&meta) {
        return Err(not_private(path));
    }
    if meta.mode() & 0o777 != 0o700 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let meta = std::fs::symlink_metadata(path)?;
    if private_bits(
        meta.uid(),
        uid,
        meta.mode(),
        meta.file_type().is_symlink(),
        meta.file_type().is_dir(),
    ) {
        Ok(())
    } else {
        Err(not_private(path))
    }
}

#[cfg(unix)]
fn real_directory(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_dir()
}

#[cfg(unix)]
fn sticky(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o1000 != 0
}

#[cfg(unix)]
fn strangers_can_replace(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    let mode = meta.mode();
    mode & 0o1000 == 0 && mode & 0o022 != 0
}

#[cfg(unix)]
fn vidcut_dir_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name == "vidcut" || name == "instances" || name.starts_with("vidcut-")
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(unix)]
fn private_bits(owner: u32, uid: u32, mode: u32, symlink: bool, dir: bool) -> bool {
    dir && !symlink && owner == uid && mode & 0o777 == 0o700
}

#[cfg(unix)]
fn not_private(path: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!("{} is not a private directory", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projectless_requests_never_route() {
        let r = Request::from_parts("schema", None).unwrap();
        let opts = RouteOptions {
            instance: Some("999999999".into()),
            ..Default::default()
        };
        assert_eq!(route(None, &opts, &r).unwrap(), Route::Headless);
    }

    #[test]
    fn a_line_past_the_limit_is_refused() {
        let mut input = std::io::Cursor::new(&b"abcdef\n"[..]);
        let err = read_capped_line(&mut input, 4).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "a line is limited to 4 bytes");
    }

    #[test]
    fn a_line_at_the_limit_is_read() {
        let mut input = std::io::Cursor::new(&b"abcd\n"[..]);
        assert_eq!(
            read_capped_line(&mut input, 4).unwrap().as_deref(),
            Some("abcd")
        );
    }

    #[cfg(unix)]
    #[test]
    fn another_owner_is_not_private() {
        assert!(!super::private_bits(0, 1000, 0o700, false, true));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_not_a_private_directory() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.path().join("vidcut-link");
        symlink(&target, &link).unwrap();
        let err = private_dir(&link).unwrap_err();
        let link = std::path::absolute(&link).unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            err.to_string(),
            format!("{} is not a private directory", link.display())
        );
        let mode = std::fs::symlink_metadata(&target)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_runtime_directory_is_mode_0700() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o1777)).unwrap();
        let dir = root.path().join("vidcut-1000");
        private_dir(&dir).unwrap();
        let meta = std::fs::symlink_metadata(&dir).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        assert_eq!(meta.uid(), unsafe { libc::getuid() });
        let parent = std::fs::symlink_metadata(root.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(parent, 0o1777);
    }

    #[cfg(unix)]
    #[test]
    fn an_open_vidcut_parent_is_closed() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let parent = root.path().join("vidcut-9");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let child = parent.join("instances");
        private_dir(&child).unwrap();
        let mode = |p: &std::path::Path| {
            std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode(&parent), 0o700);
        assert_eq!(mode(&child), 0o700);
        assert_eq!(mode(root.path()), 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn a_shared_parent_is_not_a_private_directory() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        let dir = root.path().join("instances");
        let err = private_dir(&dir).unwrap_err();
        let parent = std::path::absolute(root.path()).unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            err.to_string(),
            format!("{} is not a private directory", parent.display())
        );
        assert!(std::fs::symlink_metadata(&dir).is_err());
    }
}
