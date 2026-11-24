#[cfg(unix)]
use std::path::PathBuf;
use std::{path::Path, time::Duration};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
};
use tracing::{debug, warn};

/// IPC request sent from CLI to the running Testrix GUI instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcRequest {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// IPC response returned by the running Testrix GUI instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcResponse {
    pub ok: bool,
    pub reloaded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub(crate) type IpcCommand = (IpcRequest, oneshot::Sender<IpcResponse>);

/// Returns the platform-specific IPC transport name (`unix_domain_socket` or `named_pipe`).
pub fn ipc_transport_name() -> &'static str {
    #[cfg(unix)]
    {
        "unix_domain_socket"
    }
    #[cfg(target_os = "windows")]
    {
        "named_pipe"
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        "unsupported"
    }
}

#[cfg(unix)]
pub fn unix_socket_path() -> PathBuf {
    let base = testrix_storage::config_file_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".testrix"));
    base.join("testrix-ipc.sock")
}

#[cfg(target_os = "windows")]
pub fn windows_pipe_name() -> &'static str {
    r"\\.\pipe\testrix-ipc"
}

/// Normalizes a workspace path for cross-process comparison.
pub(crate) fn normalize_workspace_path(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = resolved.to_string_lossy().replace('\\', "/");
    let trimmed = s.trim_end_matches('/');
    #[cfg(target_os = "windows")]
    {
        trimmed.to_ascii_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        trimmed.to_string()
    }
}

async fn handle_ipc_stream<S>(stream: S, tx: mpsc::UnboundedSender<IpcCommand>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();
    let Ok(Some(line)) = lines.next_line().await else {
        return;
    };

    let req: IpcRequest = match serde_json::from_str(line.trim()) {
        Ok(req) => req,
        Err(err) => {
            let resp = IpcResponse {
                ok: false,
                reloaded: false,
                message: Some(format!("Invalid IPC JSON: {err}")),
            };
            if let Ok(json) = serde_json::to_string(&resp) {
                let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
                let _ = writer.flush().await;
            }
            return;
        }
    };

    let (reply_tx, reply_rx) = oneshot::channel();
    if tx.send((req, reply_tx)).is_err() {
        return;
    }

    let resp = match tokio::time::timeout(Duration::from_secs(5), reply_rx).await {
        Ok(Ok(resp)) => resp,
        _ => IpcResponse {
            ok: false,
            reloaded: false,
            message: Some("GUI did not respond in time".to_string()),
        },
    };

    if let Ok(json) = serde_json::to_string(&resp) {
        let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
        let _ = writer.flush().await;
    }
}

/// Starts the background Tokio IPC server (Unix Domain Socket on Unix, Named Pipe on Windows).
pub(crate) fn start_ipc_server() -> mpsc::UnboundedReceiver<IpcCommand> {
    let (tx, rx) = mpsc::unbounded_channel();

    std::thread::Builder::new()
        .name("testrix-ipc".to_string())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    warn!(error = %err, "failed creating Tokio IPC runtime");
                    return;
                }
            };

            rt.block_on(async move {
                run_ipc_server_loop(tx).await;
            });
        })
        .ok();

    rx
}

#[cfg(unix)]
async fn run_ipc_server_loop(tx: mpsc::UnboundedSender<IpcCommand>) {
    let sock_path = unix_socket_path();
    if let Some(parent) = sock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if sock_path.exists() {
        // If another instance is not actively listening, clean up the stale socket file.
        if tokio::net::UnixStream::connect(&sock_path).await.is_err() {
            let _ = std::fs::remove_file(&sock_path);
        }
    }

    let listener = match tokio::net::UnixListener::bind(&sock_path) {
        Ok(l) => l,
        Err(err) => {
            warn!(path = %sock_path.display(), error = %err, "failed binding UDS IPC listener");
            return;
        }
    };
    debug!(path = %sock_path.display(), "listening on UDS IPC socket");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    handle_ipc_stream(stream, tx_clone).await;
                });
            }
            Err(err) => {
                warn!(error = %err, "UDS IPC accept error");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg(target_os = "windows")]
async fn run_ipc_server_loop(tx: mpsc::UnboundedSender<IpcCommand>) {
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe_name = windows_pipe_name();
    let mut server = match ServerOptions::new()
        .first_pipe_instance(true)
        .create(pipe_name)
    {
        Ok(s) => s,
        Err(err) => {
            warn!(pipe = %pipe_name, error = %err, "failed creating Windows Named Pipe IPC server");
            return;
        }
    };
    debug!(pipe = %pipe_name, "listening on Windows Named Pipe IPC");

    loop {
        if let Err(err) = server.connect().await {
            warn!(error = %err, "Named Pipe IPC connect error");
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }

        let connected = server;
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            handle_ipc_stream(connected, tx_clone).await;
        });

        server = match ServerOptions::new().create(pipe_name) {
            Ok(s) => s,
            Err(err) => {
                warn!(pipe = %pipe_name, error = %err, "failed recreating Windows Named Pipe instance");
                return;
            }
        };
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
async fn run_ipc_server_loop(_tx: mpsc::UnboundedSender<IpcCommand>) {}

/// Sends a reload notification over UDS (Unix) or Named Pipe (Windows) to the running GUI app.
pub fn send_reload_notification(workspace_dir: Option<&Path>) -> Result<IpcResponse, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Failed creating Tokio runtime: {e}"))?;

    let req = IpcRequest {
        action: "reload".to_string(),
        workspace: workspace_dir.map(|p| p.to_string_lossy().into_owned()),
    };

    rt.block_on(async move { send_request_async(req).await })
}

async fn send_request_async(req: IpcRequest) -> Result<IpcResponse, String> {
    #[cfg(unix)]
    {
        let sock_path = unix_socket_path();
        let stream = tokio::net::UnixStream::connect(&sock_path)
            .await
            .map_err(|e| format!("Testrix GUI is not running ({e})"))?;
        exchange_ipc_line(stream, &req).await
    }

    #[cfg(target_os = "windows")]
    {
        use tokio::net::windows::named_pipe::ClientOptions;

        let pipe_name = windows_pipe_name();
        let mut attempts = 0;
        let client = loop {
            match ClientOptions::new().open(pipe_name) {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(231) && attempts < 10 => {
                    // ERROR_PIPE_BUSY (231): wait briefly and retry
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(e) => {
                    return Err(format!("Testrix GUI is not running ({e})"));
                }
            }
        };
        exchange_ipc_line(client, &req).await
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = req;
        Err("IPC is not supported on this platform".to_string())
    }
}

async fn exchange_ipc_line<S>(stream: S, req: &IpcRequest) -> Result<IpcResponse, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let payload =
        serde_json::to_string(req).map_err(|e| format!("Failed serializing IPC request: {e}"))?;
    let (reader, mut writer) = tokio::io::split(stream);
    writer
        .write_all(format!("{payload}\n").as_bytes())
        .await
        .map_err(|e| format!("Failed writing IPC request: {e}"))?;
    writer
        .flush()
        .await
        .map_err(|e| format!("Failed flushing IPC request: {e}"))?;

    let mut lines = BufReader::new(reader).lines();
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .map_err(|_| "Timed out waiting for GUI IPC response".to_string())?
        .map_err(|e| format!("Failed reading IPC response: {e}"))?
        .ok_or_else(|| "GUI closed IPC connection without a response".to_string())?;

    serde_json::from_str::<IpcResponse>(line.trim())
        .map_err(|e| format!("Invalid IPC response JSON: {e}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn uds_ipc_roundtrips_reload_request_and_response() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("test.sock");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            let listener = tokio::net::UnixListener::bind(&sock).unwrap();
            let (tx, mut rx) = mpsc::unbounded_channel();

            let accept_task = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                handle_ipc_stream(stream, tx).await;
            });

            let responder_task = tokio::spawn(async move {
                let (req, reply_tx) = rx.recv().await.unwrap();
                assert_eq!(req.action, "reload");
                let _ = reply_tx.send(IpcResponse {
                    ok: true,
                    reloaded: true,
                    message: Some("Reloaded".to_string()),
                });
            });

            let stream = tokio::net::UnixStream::connect(&sock).await.unwrap();
            let resp = exchange_ipc_line(
                stream,
                &IpcRequest {
                    action: "reload".to_string(),
                    workspace: Some("/tmp/ws".to_string()),
                },
            )
            .await
            .unwrap();

            assert!(resp.ok);
            assert!(resp.reloaded);
            assert_eq!(resp.message.as_deref(), Some("Reloaded"));

            accept_task.await.unwrap();
            responder_task.await.unwrap();
        });
    }
}
