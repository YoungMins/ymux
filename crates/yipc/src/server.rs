//! IPC server — listens for connections from tool processes.
//!
//! On Unix: uses a Unix domain socket.
//! On Windows (or as fallback): uses TCP on localhost.
//!
//! Traffic is tool → host only, each message answered on the same
//! connection by the handler. The host → tool push (`send_to`, and the
//! per-tool client registry it needed) went away with the file dock's
//! `ydir --dock` process: the dock is a GUI pane now and follows the active
//! pane's directory without a socket.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use crate::protocol::IpcMessage;
use crate::IpcResult;

/// Callback invoked on the client's thread for each received message.
/// Receives the deserialized message and a writer that replies to the same
/// client.
pub type MessageHandler = Box<dyn Fn(IpcMessage, &mut dyn Write) + Send + Sync>;

/// A blocking IPC server that accepts multiple clients.
pub struct IpcServer {
    /// The address string to give to clients (socket path or `tcp:host:port`).
    address: String,
    /// Signals the accept loop to stop.
    stop: Arc<AtomicBool>,
    /// Handle for the main accept thread.
    accept_thread: Option<thread::JoinHandle<()>>,
}

impl IpcServer {
    /// Start the IPC server. Returns immediately; the accept loop runs on a
    /// background thread. Each connected client is handled on its own thread.
    ///
    /// `handler` is called for every message received from any client.
    pub fn start(handler: MessageHandler) -> IpcResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);

        #[cfg(unix)]
        let (address, accept_thread) = Self::start_unix(Arc::clone(&stop), handler)?;

        #[cfg(not(unix))]
        let (address, accept_thread) = Self::start_tcp(Arc::clone(&stop), handler)?;

        Ok(Self {
            address,
            stop,
            accept_thread: Some(accept_thread),
        })
    }

    /// The address string clients should use to connect. This is the value to
    /// put in the `YMUX_IPC` environment variable.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Signal the server to stop accepting new connections. Existing client
    /// handler threads will finish naturally when their client disconnects.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // Connect to ourselves to unblock the accept() call so it can check
        // the stop flag and exit.
        #[cfg(unix)]
        {
            let _ = std::os::unix::net::UnixStream::connect(&self.address);
        }
        #[cfg(not(unix))]
        {
            if let Some(addr) = self.address.strip_prefix("tcp:") {
                let _ = std::net::TcpStream::connect(addr);
            }
        }
    }

    /// Stop the server and wait for the accept thread to finish.
    pub fn shutdown(mut self) {
        self.stop();
        if let Some(t) = self.accept_thread.take() {
            let _ = t.join();
        }
    }

    // ─── Unix implementation ─────────────────────────────────────────────

    #[cfg(unix)]
    fn start_unix(
        stop: Arc<AtomicBool>,
        handler: Arc<MessageHandler>,
    ) -> IpcResult<(String, thread::JoinHandle<()>)> {
        use std::os::unix::net::UnixListener;

        let session_id = uuid::Uuid::new_v4();
        let path = format!("/tmp/ymux-{session_id}.sock");

        // Remove stale socket if it exists.
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(false)?;

        let address = path.clone();
        let stop2 = Arc::clone(&stop);

        let handle = thread::Builder::new()
            .name("ymux-ipc-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stop2.load(Ordering::SeqCst) {
                        break;
                    }
                    match stream {
                        Ok(stream) => match stream.try_clone() {
                            Ok(writer) => Self::spawn_client(stream, writer, &handler, &stop2),
                            Err(e) => eprintln!("ipc clone error: {e}"),
                        },
                        Err(e) => {
                            if stop2.load(Ordering::SeqCst) {
                                break;
                            }
                            eprintln!("ipc accept error: {e}");
                        }
                    }
                }
                // Clean up socket file.
                let _ = std::fs::remove_file(&path);
            })?;

        Ok((address, handle))
    }

    // ─── TCP fallback (Windows / non-unix) ───────────────────────────────

    #[cfg(not(unix))]
    fn start_tcp(
        stop: Arc<AtomicBool>,
        handler: Arc<MessageHandler>,
    ) -> IpcResult<(String, thread::JoinHandle<()>)> {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")?;
        let local_addr = listener.local_addr()?;
        let address = format!("tcp:{}", local_addr);

        let stop2 = Arc::clone(&stop);

        let handle = thread::Builder::new()
            .name("ymux-ipc-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stop2.load(Ordering::SeqCst) {
                        break;
                    }
                    match stream {
                        Ok(stream) => match stream.try_clone() {
                            Ok(writer) => Self::spawn_client(stream, writer, &handler, &stop2),
                            Err(e) => eprintln!("ipc clone error: {e}"),
                        },
                        Err(e) => {
                            if stop2.load(Ordering::SeqCst) {
                                break;
                            }
                            eprintln!("ipc accept error: {e}");
                        }
                    }
                }
            })?;

        Ok((address, handle))
    }

    /// Serve one accepted connection on its own thread: read its messages
    /// until it disconnects and hand each to `handler` with the reply writer.
    fn spawn_client<R, W>(
        reader: R,
        mut writer: W,
        handler: &Arc<MessageHandler>,
        stop: &Arc<AtomicBool>,
    ) where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let handler = Arc::clone(handler);
        let stop = Arc::clone(stop);
        thread::Builder::new()
            .name("ymux-ipc-client".into())
            .spawn(move || {
                for line in BufReader::new(reader).lines() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(line) = line else { break };
                    if line.is_empty() {
                        continue;
                    }
                    match IpcMessage::from_line(&line) {
                        Ok(msg) => handler(msg, &mut writer),
                        Err(e) => eprintln!("ipc parse error: {e}"),
                    }
                }
            })
            .ok();
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock the accept loop.
        #[cfg(unix)]
        {
            let _ = std::os::unix::net::UnixStream::connect(&self.address);
        }
        #[cfg(not(unix))]
        {
            if let Some(addr) = self.address.strip_prefix("tcp:") {
                let _ = std::net::TcpStream::connect(addr);
            }
        }
        if let Some(t) = self.accept_thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IpcClient;
    use std::sync::mpsc;

    #[cfg(unix)]
    #[test]
    fn server_starts_and_stops() {
        let (tx, rx) = mpsc::channel();
        let handler: MessageHandler = Box::new(move |msg, _writer| {
            tx.send(msg).ok();
        });
        let server = IpcServer::start(handler).unwrap();
        let addr = server.address().to_string();
        assert!(addr.starts_with("/tmp/ymux-"));
        assert!(addr.ends_with(".sock"));

        // Ensure the socket file exists.
        assert!(std::path::Path::new(&addr).exists());

        server.shutdown();

        // Socket file should be cleaned up.
        assert!(!std::path::Path::new(&addr).exists());

        // No messages should have been received.
        assert!(rx.try_recv().is_err());
    }

    /// The one round trip the host relies on (the agent-hook relay): a tool
    /// sends, the handler sees the message and its reply reaches the tool.
    /// Runs on both transports, so Windows covers the server too.
    #[test]
    fn a_message_reaches_the_handler_and_its_reply_the_client() {
        let (tx, rx) = mpsc::channel();
        let handler: MessageHandler = Box::new(move |msg, writer| {
            tx.send(msg).ok();
            writer
                .write_all(IpcMessage::Ack.to_line().unwrap().as_bytes())
                .ok();
            writer.flush().ok();
        });
        let server = IpcServer::start(handler).unwrap();
        let mut client = IpcClient::connect(server.address()).unwrap();
        let hello = IpcMessage::Hello {
            tool: "y".into(),
            pane_id: "p1".into(),
        };
        client.send(&hello).unwrap();
        assert_eq!(client.recv().unwrap(), IpcMessage::Ack);
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            hello
        );
        server.shutdown();
    }
}
