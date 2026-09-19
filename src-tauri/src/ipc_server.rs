//! Starts the yipc server inside the ymux process and bridges IPC messages
//! into Tauri events so the frontend can react.
//!
//! Feature-gated behind `desktop` because it requires both `yipc` and `tauri`.

use std::io::Write;

use tauri::{AppHandle, Emitter, Manager, State};
use yipc::{IpcMessage, IpcServer, MessageHandler, AGENT_HOOK_KIND};

use crate::error::{YmuxError, YmuxResult};

/// Tauri event name emitted for every incoming IPC message.
const IPC_EVENT: &str = "ymux://ipc-message";

/// Serializable payload forwarded to the frontend via a Tauri event.
#[derive(Debug, Clone, serde::Serialize)]
struct IpcEventPayload {
    /// The raw JSON of the message (so the frontend can deserialize with its
    /// own TypeScript types).
    message: serde_json::Value,
}

/// The running IPC server, kept in Tauri state so commands can push
/// host → tool messages. It is `'static` because `start_ipc_server` leaks
/// the server for the life of the process.
pub struct IpcServerState(pub &'static IpcServer);

/// Start the IPC server on a background thread. Returns the address string
/// that should be injected as the `YMUX_IPC` environment variable into every
/// spawned PTY.
///
/// The server thread will stop automatically when the [`IpcServer`] is dropped
/// (which happens when the `AppHandle` — and thus the managed state — is
/// dropped on app exit).
pub fn start_ipc_server(app: AppHandle) -> String {
    // The handler closure takes `app` by move, so keep a handle for
    // registering the server in managed state afterwards.
    let state_handle = app.clone();
    let handler: MessageHandler = Box::new(move |msg: IpcMessage, writer: &mut dyn Write| {
        match &msg {
            // Agent-tree hook relayed by `y agent-hook`: into the registry,
            // not onto the generic frontend channel.
            IpcMessage::Event { kind, payload } if kind == AGENT_HOOK_KIND => {
                crate::commands::apply_agent_hook(&app, payload);
            }
            _ => {
                // Serialize the message to a JSON Value for the event payload.
                if let Ok(value) = serde_json::to_value(&msg) {
                    let payload = IpcEventPayload { message: value };
                    let _ = app.emit(IPC_EVENT, &payload);
                }
            }
        }

        // Always acknowledge.
        if let Ok(ack_line) = IpcMessage::Ack.to_line() {
            let _ = writer.write_all(ack_line.as_bytes());
            let _ = writer.flush();
        }
    });

    let server = IpcServer::start(handler).expect("failed to start IPC server");
    let address = server.address().to_string();

    // Leak the server into a Box so it lives for the duration of the process.
    // The Drop impl will clean up when the process exits.
    let server: &'static IpcServer = Box::leak(Box::new(server));
    state_handle.manage(IpcServerState(server));

    tracing::info!(address = %address, "IPC server started");
    address
}

/// Point the file dock's yDir at `path`. It is delivered over yipc to every
/// client registered as `ydir`, and only a yDir started with `--dock`
/// registers. Reaching nobody is not an error, because the dock may not
/// have been opened yet.
///
/// `async` so the socket write runs off the main thread: a sync command
/// runs on it, and a yDir that stopped reading could otherwise freeze the
/// window for up to `yipc::WRITE_TIMEOUT` per call.
#[tauri::command(async)]
pub fn filedock_change_dir(ipc: State<'_, IpcServerState>, path: String) -> YmuxResult<()> {
    ipc.0
        .send_to("ydir", &IpcMessage::ChangeDir { path })
        .map(|_| ())
        .map_err(|e| YmuxError::Other(format!("filedock_change_dir: {e}")))
}
