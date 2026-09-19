//! IPC server — listens for connections from tool processes.
//!
//! On Unix: uses a Unix domain socket.
//! On Windows (or as fallback): uses TCP on localhost.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::protocol::IpcMessage;
use crate::IpcResult;

/// Callback invoked on the accept thread for each received message.
/// Receives the deserialized message and a sender that can write replies back
/// to the same client.
pub type MessageHandler = Box<dyn Fn(IpcMessage, &mut dyn Write) + Send + Sync>;

/// Write half of one connected client. It is shared between that client's
/// reader thread, which writes replies, and [`IpcServer::send_to`], which
/// pushes host → tool messages.
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// A client that has identified itself with `Hello`.
struct ClientEntry {
    conn: u64,
    tool: String,
    writer: SharedWriter,
}

type ClientRegistry = Arc<Mutex<Vec<ClientEntry>>>;

/// Per-connection id, so a client can be unregistered without comparing
/// sockets.
static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// A blocking IPC server that accepts multiple clients.
pub struct IpcServer {
    /// The address string to give to clients (socket path or `tcp:host:port`).
    address: String,
    /// Signals the accept loop to stop.
    stop: Arc<AtomicBool>,
    /// Handle for the main accept thread.
    accept_thread: Option<thread::JoinHandle<()>>,
    /// Clients that sent `Hello`, by tool name. Read by `send_to`.
    clients: ClientRegistry,
}

impl IpcServer {
    /// Start the IPC server. Returns immediately; the accept loop runs on a
    /// background thread. Each connected client is handled on its own thread.
    ///
    /// `handler` is called for every message received from any client.
    pub fn start(handler: MessageHandler) -> IpcResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);
        let clients: ClientRegistry = Arc::default();

        #[cfg(unix)]
        let (address, accept_thread) =
            Self::start_unix(Arc::clone(&stop), handler, Arc::clone(&clients))?;

        #[cfg(not(unix))]
        let (address, accept_thread) =
            Self::start_tcp(Arc::clone(&stop), handler, Arc::clone(&clients))?;

        Ok(Self {
            address,
            stop,
            accept_thread: Some(accept_thread),
            clients,
        })
    }

    /// The address string clients should use to connect. This is the value to
    /// put in the `YMUX_IPC` environment variable.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Send `msg` to every connected client that introduced itself with
    /// `Hello { tool, .. }` under this tool name, and return how many it
    /// reached. Zero is not an error, because the tool may simply not be
    /// running. A client whose write fails is dropped from the registry.
    /// Its reader thread notices the dead socket on its own.
    pub fn send_to(&self, tool: &str, msg: &IpcMessage) -> IpcResult<usize> {
        let line = msg.to_line()?;
        // Clone the writers out so no socket write happens under the
        // registry lock.
        let targets: Vec<(u64, SharedWriter)> = self
            .clients
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.tool == tool)
            .map(|c| (c.conn, Arc::clone(&c.writer)))
            .collect();
        let mut sent = 0;
        let mut dead = Vec::new();
        for (conn, writer) in targets {
            let mut w = writer.lock().unwrap();
            if w.write_all(line.as_bytes()).is_ok() && w.flush().is_ok() {
                sent += 1;
            } else {
                dead.push(conn);
            }
        }
        if !dead.is_empty() {
            self.clients
                .lock()
                .unwrap()
                .retain(|c| !dead.contains(&c.conn));
        }
        Ok(sent)
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
        clients: ClientRegistry,
    ) -> IpcResult<(String, thread::JoinHandle<()>)> {
        use std::os::unix::net::UnixListener;

        let session_id = uuid::Uuid::new_v4();
        let path = format!("/tmp/ymux-{session_id}.sock");

        // Remove stale socket if it exists.
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path)?;
        // Set a timeout on accept so we can periodically check the stop flag.
        listener.set_nonblocking(false)?;

        let address = path.clone();
        let stop2 = Arc::clone(&stop);

        let handle = thread::Builder::new()
            .name("ymux-ipc-accept".into())
            .spawn(move || {
                // Set a short timeout so we wake up to check the stop flag.
                let _ = listener.set_nonblocking(false);

                for stream in listener.incoming() {
                    if stop2.load(Ordering::SeqCst) {
                        break;
                    }
                    match stream {
                        Ok(stream) => {
                            let handler = Arc::clone(&handler);
                            let stop3 = Arc::clone(&stop2);
                            let clients = Arc::clone(&clients);
                            thread::Builder::new()
                                .name("ymux-ipc-client".into())
                                .spawn(move || {
                                    let Ok(writer) = stream.try_clone() else {
                                        return;
                                    };
                                    // Box first: the unsizing coercion does
                                    // not reach through `Mutex`.
                                    let writer: Box<dyn Write + Send> = Box::new(writer);
                                    let writer: SharedWriter = Arc::new(Mutex::new(writer));
                                    Self::serve_client(stream, writer, &handler, &stop3, &clients);
                                })
                                .ok();
                        }
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
        clients: ClientRegistry,
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
                        Ok(stream) => {
                            let handler = Arc::clone(&handler);
                            let stop3 = Arc::clone(&stop2);
                            let clients = Arc::clone(&clients);
                            thread::Builder::new()
                                .name("ymux-ipc-client".into())
                                .spawn(move || {
                                    let Ok(writer) = stream.try_clone() else {
                                        return;
                                    };
                                    // Box first: the unsizing coercion does
                                    // not reach through `Mutex`.
                                    let writer: Box<dyn Write + Send> = Box::new(writer);
                                    let writer: SharedWriter = Arc::new(Mutex::new(writer));
                                    Self::serve_client(stream, writer, &handler, &stop3, &clients);
                                })
                                .ok();
                        }
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

    /// Read one client's messages until it disconnects, handing each to
    /// `handler`. A `Hello` registers the client under its tool name so
    /// `send_to` can reach it. The registration is removed when the loop
    /// ends, whether by end of stream, error or server stop.
    fn serve_client<R: Read>(
        reader: R,
        writer: SharedWriter,
        handler: &MessageHandler,
        stop: &AtomicBool,
        clients: &ClientRegistry,
    ) {
        let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
        for line in BufReader::new(reader).lines() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(line) = line else { break };
            if line.is_empty() {
                continue;
            }
            let msg = match IpcMessage::from_line(&line) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("ipc parse error: {e}");
                    continue;
                }
            };
            // The registry guard drops at the end of this block, before the
            // writer lock below. `send_to` never holds both at once either,
            // so the two paths cannot deadlock each other.
            if let IpcMessage::Hello { tool, .. } = &msg {
                let mut reg = clients.lock().unwrap();
                reg.retain(|c| c.conn != conn);
                reg.push(ClientEntry {
                    conn,
                    tool: tool.clone(),
                    writer: Arc::clone(&writer),
                });
            }
            let mut w = writer.lock().unwrap();
            handler(msg, &mut **w);
        }
        clients.lock().unwrap().retain(|c| c.conn != conn);
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

    use crate::IpcClient;
    use std::time::{Duration, Instant};

    /// A server whose handler acknowledges every message, like ymux's.
    fn ack_server() -> IpcServer {
        let handler: MessageHandler = Box::new(|_msg, writer| {
            let ack = IpcMessage::Ack.to_line().unwrap();
            writer.write_all(ack.as_bytes()).ok();
            writer.flush().ok();
        });
        IpcServer::start(handler).unwrap()
    }

    fn hello(tool: &str) -> IpcMessage {
        IpcMessage::Hello {
            tool: tool.into(),
            pane_id: format!("{tool}-pane"),
        }
    }

    #[test]
    fn send_to_reaches_only_the_named_tool() {
        let server = ack_server();
        let mut ydir = IpcClient::connect(server.address()).unwrap();
        let mut ymon = IpcClient::connect(server.address()).unwrap();
        ydir.send(&hello("ydir")).unwrap();
        ymon.send(&hello("ymon")).unwrap();
        // The server registers a client before handing its Hello to the
        // handler, so once the Ack is back the client is routable.
        assert_eq!(ydir.recv().unwrap(), IpcMessage::Ack);
        assert_eq!(ymon.recv().unwrap(), IpcMessage::Ack);

        let msg = IpcMessage::ChangeDir {
            path: "/some/dir".into(),
        };
        assert_eq!(server.send_to("ydir", &msg).unwrap(), 1);
        assert_eq!(ydir.recv().unwrap(), msg);
        server.shutdown();
    }

    #[test]
    fn send_to_an_absent_tool_reaches_nobody() {
        let server = ack_server();
        let msg = IpcMessage::ChangeDir { path: "/x".into() };
        assert_eq!(server.send_to("ydir", &msg).unwrap(), 0);
        server.shutdown();
    }

    #[test]
    fn disconnected_client_is_unregistered() {
        let server = ack_server();
        let mut c = IpcClient::connect(server.address()).unwrap();
        c.send(&hello("ydir")).unwrap();
        assert_eq!(c.recv().unwrap(), IpcMessage::Ack);
        drop(c);

        let deadline = Instant::now() + Duration::from_secs(2);
        while server.send_to("ydir", &IpcMessage::Ack).unwrap() != 0 {
            assert!(
                Instant::now() < deadline,
                "client still registered after it disconnected"
            );
            thread::sleep(Duration::from_millis(20));
        }
        server.shutdown();
    }
}
