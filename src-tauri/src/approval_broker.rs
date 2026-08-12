//! Authenticated app-owned Unix broker for local agent clients.
//!
//! The MCP helper is intentionally unable to open SQLite. Every operation is a
//! bounded request to this broker, which authenticates the registered client,
//! verifies the peer belongs to the same macOS user, and delegates policy to the
//! in-process Context Pass service.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

use crate::context_pass::{AgentAccess, ContextOptions};
use crate::db::Db;

const MAX_BROKER_MESSAGE: usize = 64 * 1024;

pub struct AgentAccessState(pub Arc<AgentAccess>);

#[derive(Debug, Deserialize)]
struct BrokerEnvelope {
    version: u32,
    client_id: String,
    secret: String,
    #[serde(default)]
    runtime_name: Option<String>,
    op: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Deserialize)]
struct RequestContextArgs {
    purpose: String,
    query: String,
    #[serde(default)]
    options: ContextOptions,
}

#[derive(Debug, Deserialize)]
struct RequestStatusArgs {
    request_id: String,
}

#[derive(Debug, Deserialize)]
struct ReadPassArgs {
    pass_id: String,
    #[serde(default)]
    cursor: usize,
}

pub fn spawn(app: tauri::AppHandle, access: Arc<AgentAccess>) -> Result<()> {
    let path = access.socket_path();
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            bail!("another Noted agent broker is already listening");
        }
        std::fs::remove_file(&path).context("could not remove a stale agent broker socket")?;
    }
    let listener = UnixListener::bind(&path).context("could not bind the agent broker socket")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new()
        .name("noted-agent-broker".into())
        .spawn(move || {
            for connection in listener.incoming() {
                let Ok(stream) = connection else {
                    continue;
                };
                let app = app.clone();
                let access = access.clone();
                std::thread::spawn(move || {
                    if let Err(error) = handle_connection(&app, &access, stream) {
                        eprintln!("[noted] agent broker request rejected: {error}");
                    }
                });
            }
        })?;
    Ok(())
}

fn handle_connection(
    app: &tauri::AppHandle,
    access: &Arc<AgentAccess>,
    mut stream: UnixStream,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    verify_peer_user(&stream)?;

    let response = match read_message(&mut stream)
        .and_then(|bytes| serde_json::from_slice::<BrokerEnvelope>(&bytes).map_err(Into::into))
        .and_then(|envelope| dispatch(app, access, envelope))
    {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    };
    let mut encoded = serde_json::to_vec(&response)?;
    encoded.push(b'\n');
    stream.write_all(&encoded)?;
    stream.flush()?;
    Ok(())
}

fn dispatch(
    app: &tauri::AppHandle,
    access: &Arc<AgentAccess>,
    envelope: BrokerEnvelope,
) -> Result<Value> {
    if envelope.version != 1 {
        bail!("unsupported Noted broker protocol version");
    }
    let client = access.authenticate(&envelope.client_id, &envelope.secret)?;
    access.mark_seen(&client.id);
    match envelope.op.as_str() {
        "ping" => Ok(json!({ "ready": true })),
        "request_context" => {
            let args: RequestContextArgs = serde_json::from_value(envelope.args)
                .context("invalid request_context arguments")?;
            let status = {
                let state = app.state::<Db>();
                let conn = state.0.lock().unwrap();
                access.request_context(
                    &conn,
                    &client,
                    envelope.runtime_name,
                    &args.purpose,
                    &args.query,
                    args.options,
                )?
            };
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            let _ = app.emit("agent-context-requested", status.request_id.clone());
            serde_json::to_value(status).map_err(Into::into)
        }
        "context_request_status" => {
            let args: RequestStatusArgs = serde_json::from_value(envelope.args)
                .context("invalid context_request_status arguments")?;
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            serde_json::to_value(access.request_status(&conn, &client.id, &args.request_id)?)
                .map_err(Into::into)
        }
        "read_context_pass" => {
            let args: ReadPassArgs = serde_json::from_value(envelope.args)
                .context("invalid read_context_pass arguments")?;
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            serde_json::to_value(access.read_pass(&conn, &client.id, &args.pass_id, args.cursor)?)
                .map_err(Into::into)
        }
        _ => Err(anyhow!("unknown Noted broker operation")),
    }
}

fn read_message(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(4096);
    let mut buffer = [0u8; 4096];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let slice = &buffer[..read];
        if let Some(newline) = slice.iter().position(|byte| *byte == b'\n') {
            output.extend_from_slice(&slice[..newline]);
            break;
        }
        output.extend_from_slice(slice);
        if output.len() > MAX_BROKER_MESSAGE {
            bail!("agent broker request is too large");
        }
    }
    if output.is_empty() {
        bail!("empty agent broker request");
    }
    if output.len() > MAX_BROKER_MESSAGE {
        bail!("agent broker request is too large");
    }
    Ok(output)
}

#[cfg(target_os = "macos")]
fn verify_peer_user(stream: &UnixStream) -> Result<()> {
    let mut credentials: libc::xucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::xucred>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERCRED,
            &mut credentials as *mut _ as *mut libc::c_void,
            &mut length,
        )
    };
    if status != 0 {
        return Err(std::io::Error::last_os_error()).context("could not verify agent peer user");
    }
    let current = unsafe { libc::geteuid() };
    if credentials.cr_uid != current {
        bail!("agent broker peer belongs to a different macOS user");
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn verify_peer_user(stream: &UnixStream) -> Result<()> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut credentials as *mut _ as *mut libc::c_void,
            &mut length,
        )
    };
    if status != 0 {
        return Err(std::io::Error::last_os_error()).context("could not verify agent peer user");
    }
    if credentials.uid != unsafe { libc::geteuid() } {
        bail!("agent broker peer belongs to a different user");
    }
    Ok(())
}
