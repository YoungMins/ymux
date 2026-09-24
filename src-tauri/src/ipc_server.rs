//! Starts the yipc server inside the ymux process and bridges IPC messages
//! into Tauri events so the frontend can react.
//!
//! Feature-gated behind `desktop` because it requires both `yipc` and `tauri`.

use std::io::Write;

use tauri::{AppHandle, Emitter};
use yipc::{IpcMessage, IpcServer, MessageHandler, AGENT_HOOK_KIND};

/// Tauri event name emitted for every incoming IPC message.
const IPC_EVENT: &str = "ymux://ipc-message";

/// Tauri event carrying the path a `ydir --dock` asked ymux to open.
const OPEN_FILE_EVENT: &str = "ymux:open-file";

/// Serializable payload forwarded to the frontend via a Tauri event.
#[derive(Debug, Clone, serde::Serialize)]
struct IpcEventPayload {
    /// The raw JSON of the message (so the frontend can deserialize with its
    /// own TypeScript types).
    message: serde_json::Value,
}

/// Start the IPC server on a background thread. Returns the address string
/// that should be injected as the `YMUX_IPC` environment variable into every
/// spawned PTY.
///
/// The server is leaked for the life of the process: `IpcServer`'s `Drop`
/// stops the accept thread, and the agent-hook relay (`y agent-hook`) must
/// keep reaching it for as long as ymux runs.
pub fn start_ipc_server(app: AppHandle) -> String {
    let handler: MessageHandler = Box::new(move |msg: IpcMessage, writer: &mut dyn Write| {
        match &msg {
            // Agent-tree hook relayed by `y agent-hook`: into the registry,
            // not onto the generic frontend channel.
            IpcMessage::Event { kind, payload } if kind == AGENT_HOOK_KIND => {
                crate::commands::apply_agent_hook(&app, payload);
            }
            // A `ydir --dock` pressed Enter on a file: hand the path to the
            // frontend, which puts it in the viewer tab of the active pane.
            // The dock itself is a GUI pane now and calls that directly; this
            // route goes with the viewer tab's PTY in step 3.
            IpcMessage::Event { kind, .. } if kind == yipc::OPEN_FILE_KIND => {
                if let Some(path) = yipc::open_file_path(&msg) {
                    let _ = app.emit(OPEN_FILE_EVENT, path);
                }
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

    // Leak the server so it lives for the duration of the process; dropping
    // it would stop the accept loop.
    let _: &'static IpcServer = Box::leak(Box::new(server));

    tracing::info!(address = %address, "IPC server started");
    address
}
