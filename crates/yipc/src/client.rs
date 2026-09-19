//! IPC client — used by tools running inside PTY panes to communicate with
//! the ymux host process.

use std::io::{BufRead, BufReader, Write};

use crate::protocol::IpcMessage;
use crate::{IpcError, IpcResult};

/// A blocking IPC client that communicates with the ymux server.
pub struct IpcClient {
    #[cfg(unix)]
    reader: BufReader<std::os::unix::net::UnixStream>,
    #[cfg(unix)]
    writer: std::os::unix::net::UnixStream,

    #[cfg(not(unix))]
    reader: BufReader<std::net::TcpStream>,
    #[cfg(not(unix))]
    writer: std::net::TcpStream,
}

impl IpcClient {
    /// Connect to the IPC server at the given address.
    ///
    /// The address format is:
    /// - Unix: a filesystem path (e.g. `/tmp/ymux-{uuid}.sock`)
    /// - Windows/fallback: `tcp:host:port`
    pub fn connect(address: &str) -> IpcResult<Self> {
        #[cfg(unix)]
        {
            Self::connect_unix(address)
        }
        #[cfg(not(unix))]
        {
            Self::connect_tcp(address)
        }
    }

    /// Connect using the `YMUX_IPC` environment variable.
    pub fn from_env() -> IpcResult<Self> {
        let addr = std::env::var("YMUX_IPC").map_err(|_| IpcError::EnvNotSet)?;
        Self::connect(&addr)
    }

    /// Send a message to the server.
    pub fn send(&mut self, msg: &IpcMessage) -> IpcResult<()> {
        let line = msg.to_line()?;
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    /// Read one message from the server (blocks until available).
    pub fn recv(&mut self) -> IpcResult<IpcMessage> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            return Err(IpcError::ConnectionClosed);
        }
        let msg = IpcMessage::from_line(&line)?;
        Ok(msg)
    }

    /// Bound how long [`recv`](Self::recv) blocks (`None` = forever, the
    /// default). A timed-out `recv` returns [`IpcError::Io`].
    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> IpcResult<()> {
        self.reader.get_ref().set_read_timeout(timeout)?;
        Ok(())
    }

    // ─── Unix implementation ─────────────────────────────────────────────

    #[cfg(unix)]
    fn connect_unix(address: &str) -> IpcResult<Self> {
        use std::os::unix::net::UnixStream;
        let stream = UnixStream::connect(address)?;
        let writer = stream.try_clone()?;
        let reader = BufReader::new(stream);
        Ok(Self { reader, writer })
    }

    // ─── TCP fallback ────────────────────────────────────────────────────

    #[cfg(not(unix))]
    fn connect_tcp(address: &str) -> IpcResult<Self> {
        use std::net::TcpStream;
        let addr = address.strip_prefix("tcp:").unwrap_or(address);
        let stream = TcpStream::connect(addr)?;
        let writer = stream.try_clone()?;
        let reader = BufReader::new(stream);
        Ok(Self { reader, writer })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{IpcServer, MessageHandler};
    use std::sync::mpsc;

    #[cfg(unix)]
    #[test]
    fn client_connect_and_exchange() {
        let (tx, rx) = mpsc::channel();
        let handler: MessageHandler = Box::new(move |msg, writer| {
            tx.send(msg).ok();
            // Send back an Ack
            let ack = IpcMessage::Ack.to_line().unwrap();
            writer.write_all(ack.as_bytes()).ok();
            writer.flush().ok();
        });

        let server = IpcServer::start(handler).unwrap();
        let addr = server.address().to_string();

        // Give the server a moment to start listening.
        std::thread::sleep(std::time::Duration::from_millis(50));

        let mut client = IpcClient::connect(&addr).unwrap();

        // Send Hello
        let hello = IpcMessage::Hello {
            tool: "test-tool".into(),
            pane_id: "pane-1".into(),
        };
        client.send(&hello).unwrap();

        // Server should have received it
        let received = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(received, hello);

        // Client should receive the Ack
        let reply = client.recv().unwrap();
        assert_eq!(reply, IpcMessage::Ack);

        drop(client);
        server.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn multiple_clients() {
        let (tx, rx) = mpsc::channel();
        let handler: MessageHandler = Box::new(move |msg, writer| {
            tx.send(msg).ok();
            let ack = IpcMessage::Ack.to_line().unwrap();
            writer.write_all(ack.as_bytes()).ok();
            writer.flush().ok();
        });

        let server = IpcServer::start(handler).unwrap();
        let addr = server.address().to_string();
        std::thread::sleep(std::time::Duration::from_millis(50));

        let mut client1 = IpcClient::connect(&addr).unwrap();
        let mut client2 = IpcClient::connect(&addr).unwrap();

        let msg1 = IpcMessage::Hello {
            tool: "tool1".into(),
            pane_id: "p1".into(),
        };
        let msg2 = IpcMessage::Hello {
            tool: "tool2".into(),
            pane_id: "p2".into(),
        };

        client1.send(&msg1).unwrap();
        client2.send(&msg2).unwrap();

        // Both messages received
        let mut received = Vec::new();
        for _ in 0..2 {
            received.push(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap());
        }
        assert!(received.contains(&msg1));
        assert!(received.contains(&msg2));

        // Both get acks
        assert_eq!(client1.recv().unwrap(), IpcMessage::Ack);
        assert_eq!(client2.recv().unwrap(), IpcMessage::Ack);

        drop(client1);
        drop(client2);
        server.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn broken_connection_handled_gracefully() {
        let (tx, _rx) = mpsc::channel();
        let handler: MessageHandler = Box::new(move |msg, _writer| {
            tx.send(msg).ok();
        });

        let server = IpcServer::start(handler).unwrap();
        let addr = server.address().to_string();
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Connect and immediately drop — should not panic the server.
        let client = IpcClient::connect(&addr).unwrap();
        drop(client);

        // Server should still work for a new client.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut client2 = IpcClient::connect(&addr).unwrap();
        let msg = IpcMessage::Ack;
        client2.send(&msg).unwrap();

        drop(client2);
        server.shutdown();
    }

    #[test]
    fn from_env_returns_error_when_not_set() {
        // Ensure the env var is not set (it shouldn't be in test).
        std::env::remove_var("YMUX_IPC");
        let result = IpcClient::from_env();
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            matches!(err, IpcError::EnvNotSet),
            "expected EnvNotSet, got: {err}"
        );
    }

    /// `y agent-hook` waits for the host's Ack but must never hang Claude
    /// Code when the host doesn't answer.
    #[test]
    fn recv_honours_the_read_timeout() {
        let handler: MessageHandler = Box::new(|_msg, _writer| {}); // never replies
        let server = IpcServer::start(handler).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut client = IpcClient::connect(server.address()).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .unwrap();
        client.send(&IpcMessage::Ack).unwrap();
        let started = std::time::Instant::now();
        assert!(client.recv().is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        drop(client);
        server.shutdown();
    }
}
