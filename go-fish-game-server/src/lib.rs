use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub address: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            address: "127.0.0.1:9001".parse().unwrap(),
        }
    }
}

#[derive(Debug)]
pub enum DisconnectReason {
    Clean,
    ForceClosed,
    Error(String),
}

pub enum ServerMessage {
    Text(String),
    Disconnect,
}

pub enum ManagerCommand {
    Shutdown,
}

pub enum ClientEvent<S = tokio::net::TcpStream>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    Connected {
        address: SocketAddr,
        tx: mpsc::Sender<ServerMessage>,
        ws: WebSocketStream<S>,
    },
    Message {
        address: SocketAddr,
        text: String,
    },
    Disconnected {
        address: SocketAddr,
        reason: DisconnectReason,
    },
}

pub struct ClientHandle {
    pub tx: mpsc::Sender<ServerMessage>,
}

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, instrument, warn};

#[instrument(skip(ws, event_tx, msg_rx), fields(%address))]
pub async fn run_connection_handler<S, T>(
    address: SocketAddr,
    mut ws: WebSocketStream<S>,
    event_tx: mpsc::Sender<ClientEvent<T>>,
    mut msg_rx: mpsc::Receiver<ServerMessage>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    loop {
        tokio::select! {
            frame = ws.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        debug!(%text, "ClientMessage received");
                        if event_tx.send(ClientEvent::Message { address, text: text.to_string() }).await.is_err() {
                            // Manager is gone, exit cleanly
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        let _ = event_tx.send(ClientEvent::Disconnected {
                            address,
                            reason: DisconnectReason::Clean,
                        }).await;
                        break;
                    }
                    Some(Ok(_)) => {
                        // Non-text frame (binary, ping, pong, etc.) — ignore
                        continue;
                    }
                    Some(Err(e)) => {
                        error!(error = %e, "WebSocket stream error");
                        let _ = event_tx.send(ClientEvent::Disconnected {
                            address,
                            reason: DisconnectReason::Error(e.to_string()),
                        }).await;
                        break;
                    }
                    None => {
                        // Stream ended without a Close frame (force close)
                        info!("WebSocket stream closed without Close frame (force close)");
                        let _ = event_tx.send(ClientEvent::Disconnected {
                            address,
                            reason: DisconnectReason::ForceClosed,
                        }).await;
                        break;
                    }
                }
            }
            msg = msg_rx.recv() => {
                match msg {
                    None => {
                        // Handle dropped — exit cleanly
                        break;
                    }
                    Some(ServerMessage::Disconnect) => {
                        let _ = ws.send(Message::Close(None)).await;
                        break;
                    }
                    Some(ServerMessage::Text(t)) => {
                        if ws.send(Message::Text(t.into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    }
}

pub struct ConnectionManager<S = tokio::net::TcpStream>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    clients: HashMap<SocketAddr, ClientHandle>,
    event_rx: mpsc::Receiver<ClientEvent<S>>,
    event_tx: mpsc::Sender<ClientEvent<S>>,
    command_rx: mpsc::Receiver<ManagerCommand>,
    command_tx: mpsc::Sender<ManagerCommand>,
}

impl<S> ConnectionManager<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel::<ClientEvent<S>>(64);
        let (command_tx, command_rx) = mpsc::channel::<ManagerCommand>(8);
        ConnectionManager {
            clients: HashMap::new(),
            event_rx,
            event_tx,
            command_rx,
            command_tx,
        }
    }

    pub fn event_tx(&self) -> mpsc::Sender<ClientEvent<S>> {
        self.event_tx.clone()
    }

    pub fn command_tx(&self) -> mpsc::Sender<ManagerCommand> {
        self.command_tx.clone()
    }

    #[instrument(skip(self))]
    pub async fn run(mut self) {
        loop {
            tokio::select! {
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(ManagerCommand::Shutdown) | None => {
                            // Send Disconnect to all connected clients, then exit
                            for (address, handle) in &self.clients {
                                if handle.tx.send(ServerMessage::Disconnect).await.is_err() {
                                    warn!(%address, "failed to send Disconnect during shutdown");
                                }
                            }
                            break;
                        }
                    }
                }
                event = self.event_rx.recv() => {
                    match event {
                        None => break,
                        Some(ClientEvent::Connected { address, tx, ws }) => {
                            let (handler_tx, handler_rx) = mpsc::channel::<ServerMessage>(32);
                            self.clients.insert(address, ClientHandle { tx: handler_tx });
                            let event_tx = self.event_tx.clone();
                            tokio::spawn(run_connection_handler(address, ws, event_tx, handler_rx));
                            drop(tx);
                            info!(%address, "client connected");
                        }
                        Some(ClientEvent::Message { address, text }) => {
                            if let Some(handle) = self.clients.get(&address) {
                                if handle.tx.send(ServerMessage::Text(text)).await.is_err() {
                                    warn!(%address, "failed to send message to client handler");
                                }
                            } else {
                                warn!(%address, "received message from unknown client");
                            }
                        }
                        Some(ClientEvent::Disconnected { address, reason }) => {
                            self.clients.remove(&address);
                            info!(%address, reason = ?reason, "client disconnected");
                        }
                    }
                }
            }
        }
    }
}

pub async fn run_tcp_listener(
    addr: SocketAddr,
    event_tx: mpsc::Sender<ClientEvent>,
    command_rx: mpsc::Receiver<ManagerCommand>,
) {
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!(error = %e, "Failed to bind TCP listener");
            return;
        }
    };
    info!(%addr, "TCP listener bound");
    run_tcp_listener_inner(listener, event_tx, command_rx).await
}

#[instrument(skip(event_tx, command_rx))]
async fn run_tcp_listener_inner(
    listener: tokio::net::TcpListener,
    event_tx: mpsc::Sender<ClientEvent>,
    mut command_rx: mpsc::Receiver<ManagerCommand>,
) {
    let addr = listener.local_addr().unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
    loop {
        tokio::select! {
            cmd = command_rx.recv() => {
                match cmd {
                    Some(ManagerCommand::Shutdown) | None => {
                        info!(%addr, "TCP listener shutting down");
                        break;
                    }
                }
            }
            accept = listener.accept() => {
                let (stream, address) = match accept {
                    Ok(pair) => pair,
                    Err(e) => {
                        error!(error = %e, "Failed to accept TCP connection");
                        continue;
                    }
                };

                match tokio_tungstenite::accept_async(stream).await {
                    Ok(ws) => {
                        let (tx, _rx) = mpsc::channel::<ServerMessage>(32);
                        if event_tx
                            .send(ClientEvent::Connected { address, tx, ws })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        error!(%address, error = %e, "WebSocket handshake failed");
                    }
                }
            }
        }
    }
}

pub async fn run(config: Config) -> Result<(), anyhow::Error> {
    let manager = ConnectionManager::new();
    let event_tx = manager.event_tx();
    let command_tx = manager.command_tx();
    let (listener_cmd_tx, listener_cmd_rx) = mpsc::channel::<ManagerCommand>(1);
    tokio::spawn(run_tcp_listener(config.address, event_tx, listener_cmd_rx));

    tokio::select! {
        _ = manager.run() => {}
        _ = tokio::signal::ctrl_c() => {
            info!("received Ctrl+C, shutting down gracefully");
            let _ = listener_cmd_tx.send(ManagerCommand::Shutdown).await;
            let _ = command_tx.send(ManagerCommand::Shutdown).await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::SinkExt;
    use proptest::prelude::*;
    use std::net::SocketAddr;
    use tokio::io::duplex;
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};
    use tokio_tungstenite::WebSocketStream;
    use tungstenite::protocol::Role;

    /// Helper: create a server-side WebSocketStream backed by an in-memory duplex,
    /// and return the client-side half as a raw WebSocketStream (client role).
    async fn make_ws_pair() -> (
        WebSocketStream<tokio::io::DuplexStream>,
        WebSocketStream<tokio::io::DuplexStream>,
    ) {
        let (server_io, client_io) = duplex(65536);
        let server_ws =
            WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
        let client_ws =
            WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        (server_ws, client_ws)
    }

    fn test_addr() -> SocketAddr {
        "127.0.0.1:12345".parse().unwrap()
    }

    // -------------------------------------------------------------------------
    // Test: binary frame produces no ClientEvent::Message (Requirement 2.2)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn binary_frame_produces_no_message_event() {
        let (server_ws, mut client_ws) = make_ws_pair().await;
        let (event_tx, mut event_rx) = mpsc::channel::<ClientEvent>(16);
        let (_msg_tx, msg_rx) = mpsc::channel::<ServerMessage>(16);

        let addr = test_addr();
        let handler = tokio::spawn(run_connection_handler(
            addr,
            server_ws,
            event_tx,
            msg_rx,
        ));

        // Send a binary frame, then a Close frame to terminate the handler
        client_ws
            .send(tungstenite::Message::Binary(vec![1, 2, 3].into()))
            .await
            .unwrap();
        client_ws
            .send(tungstenite::Message::Close(None))
            .await
            .unwrap();

        timeout(Duration::from_secs(2), handler)
            .await
            .expect("handler timed out")
            .expect("handler panicked");

        // Drain all events — none should be ClientEvent::Message
        let mut got_message = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, ClientEvent::Message { .. }) {
                got_message = true;
            }
        }
        assert!(!got_message, "binary frame should not produce a ClientEvent::Message");
    }

    // -------------------------------------------------------------------------
    // Test: ping frame produces no ClientEvent::Message (Requirement 2.2)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn ping_frame_produces_no_message_event() {
        let (server_ws, mut client_ws) = make_ws_pair().await;
        let (event_tx, mut event_rx) = mpsc::channel::<ClientEvent>(16);
        let (_msg_tx, msg_rx) = mpsc::channel::<ServerMessage>(16);

        let addr = test_addr();
        let handler = tokio::spawn(run_connection_handler(
            addr,
            server_ws,
            event_tx,
            msg_rx,
        ));

        // Send a ping frame, then a Close frame to terminate the handler
        client_ws
            .send(tungstenite::Message::Ping(vec![].into()))
            .await
            .unwrap();
        client_ws
            .send(tungstenite::Message::Close(None))
            .await
            .unwrap();

        timeout(Duration::from_secs(2), handler)
            .await
            .expect("handler timed out")
            .expect("handler panicked");

        // Drain all events — none should be ClientEvent::Message
        let mut got_message = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, ClientEvent::Message { .. }) {
                got_message = true;
            }
        }
        assert!(!got_message, "ping frame should not produce a ClientEvent::Message");
    }

    // -------------------------------------------------------------------------
    // Test: DisconnectReason::Clean is sent when Close frame received (Requirement 4.1)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn close_frame_sends_clean_disconnect() {
        let (server_ws, mut client_ws) = make_ws_pair().await;
        let (event_tx, mut event_rx) = mpsc::channel::<ClientEvent>(16);
        let (_msg_tx, msg_rx) = mpsc::channel::<ServerMessage>(16);

        let addr = test_addr();
        let handler = tokio::spawn(run_connection_handler(
            addr,
            server_ws,
            event_tx,
            msg_rx,
        ));

        client_ws
            .send(tungstenite::Message::Close(None))
            .await
            .unwrap();

        timeout(Duration::from_secs(2), handler)
            .await
            .expect("handler timed out")
            .expect("handler panicked");

        // Find the Disconnected event and check its reason
        let mut found_clean = false;
        while let Ok(event) = event_rx.try_recv() {
            if let ClientEvent::Disconnected { reason: DisconnectReason::Clean, .. } = event {
                found_clean = true;
            }
        }
        assert!(found_clean, "expected DisconnectReason::Clean after Close frame");
    }

    // -------------------------------------------------------------------------
    // Helper: spin up a ConnectionManager, return (event_tx, command_tx, handle)
    // -------------------------------------------------------------------------
    fn start_manager() -> (
        mpsc::Sender<ClientEvent<tokio::io::DuplexStream>>,
        mpsc::Sender<ManagerCommand>,
        tokio::task::JoinHandle<()>,
    ) {
        let manager: ConnectionManager<tokio::io::DuplexStream> = ConnectionManager::new();
        let event_tx = manager.event_tx();
        let command_tx = manager.command_tx();
        let handle = tokio::spawn(manager.run());
        (event_tx, command_tx, handle)
    }

    // Connect a duplex-backed client to the manager, returning the client-side WS.
    async fn connect_client(
        event_tx: &mpsc::Sender<ClientEvent<tokio::io::DuplexStream>>,
        addr: SocketAddr,
    ) -> WebSocketStream<tokio::io::DuplexStream> {
        let (server_io, client_io) = duplex(65536);
        let server_ws = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
        let client_ws = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let (tx, _rx) = mpsc::channel::<ServerMessage>(1);
        event_tx
            .send(ClientEvent::Connected { address: addr, tx, ws: server_ws })
            .await
            .unwrap();
        client_ws
    }

    // -------------------------------------------------------------------------
    // Test: echo round-trip — manager echoes text back to sender (Req 3.1, 3.2)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn echo_round_trip() {
        let (event_tx, command_tx, manager_handle) = start_manager();
        let addr: SocketAddr = "127.0.0.1:10001".parse().unwrap();

        let mut client_ws = connect_client(&event_tx, addr).await;
        // Give the manager a moment to register the client and spawn its handler
        tokio::time::sleep(Duration::from_millis(10)).await;

        client_ws.send(tungstenite::Message::Text("hello".into())).await.unwrap();

        let reply = timeout(Duration::from_secs(2), client_ws.next()).await
            .expect("timed out waiting for echo")
            .expect("stream ended")
            .expect("ws error");

        assert_eq!(reply, tungstenite::Message::Text("hello".into()));

        command_tx.send(ManagerCommand::Shutdown).await.unwrap();
        let _ = timeout(Duration::from_secs(2), manager_handle).await;
    }

    // -------------------------------------------------------------------------
    // Test: echo isolation — only the sender receives the echo (Req 3.3, 5.2)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn echo_isolation() {
        let (event_tx, command_tx, manager_handle) = start_manager();
        let addr_a: SocketAddr = "127.0.0.1:10002".parse().unwrap();
        let addr_b: SocketAddr = "127.0.0.1:10003".parse().unwrap();

        let mut client_a = connect_client(&event_tx, addr_a).await;
        let mut client_b = connect_client(&event_tx, addr_b).await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        client_a.send(tungstenite::Message::Text("only-a".into())).await.unwrap();

        // client_a should receive the echo
        let reply = timeout(Duration::from_secs(2), client_a.next()).await
            .expect("timed out waiting for echo on client_a")
            .expect("stream ended")
            .expect("ws error");
        assert_eq!(reply, tungstenite::Message::Text("only-a".into()));

        // client_b should receive nothing
        let nothing = timeout(Duration::from_millis(100), client_b.next()).await;
        assert!(nothing.is_err(), "client_b should not receive any message");

        command_tx.send(ManagerCommand::Shutdown).await.unwrap();
        let _ = timeout(Duration::from_secs(2), manager_handle).await;
    }

    // -------------------------------------------------------------------------
    // Test: disconnection removes client — registry cleared after disconnect (Req 4.1, 4.2)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn disconnection_removes_client() {
        let (event_tx, command_tx, manager_handle) = start_manager();
        let addr: SocketAddr = "127.0.0.1:10004".parse().unwrap();

        let mut client_ws = connect_client(&event_tx, addr).await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Disconnect cleanly via Close frame
        client_ws.send(tungstenite::Message::Close(None)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // After disconnect, a message to that address should be a no-op (WARN logged, no echo)
        event_tx.send(ClientEvent::Message { address: addr, text: "ghost".into() }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // The client WS should be closed — next() returns None, a Close frame, or a protocol error
        let next = timeout(Duration::from_millis(200), client_ws.next()).await;
        match next {
            Ok(Some(Ok(tungstenite::Message::Close(_)))) | Ok(None) | Err(_) => {}
            Ok(Some(Err(_))) => {} // Protocol error is fine — handler dropped the stream
            Ok(Some(Ok(tungstenite::Message::Text(t)))) => {
                panic!("disconnected client received unexpected message: {t}");
            }
            other => panic!("unexpected: {other:?}"),
        }

        command_tx.send(ManagerCommand::Shutdown).await.unwrap();
        let _ = timeout(Duration::from_secs(2), manager_handle).await;
    }

    // -------------------------------------------------------------------------
    // Test: disconnect does not affect remaining clients (Req 4.4)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn disconnect_does_not_affect_remaining_clients() {
        let (event_tx, command_tx, manager_handle) = start_manager();
        let addr_a: SocketAddr = "127.0.0.1:10005".parse().unwrap();
        let addr_b: SocketAddr = "127.0.0.1:10006".parse().unwrap();

        let mut client_a = connect_client(&event_tx, addr_a).await;
        let mut client_b = connect_client(&event_tx, addr_b).await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Disconnect client_a
        client_a.send(tungstenite::Message::Close(None)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // client_b should still echo correctly
        client_b.send(tungstenite::Message::Text("still-here".into())).await.unwrap();
        let reply = timeout(Duration::from_secs(2), client_b.next()).await
            .expect("timed out waiting for echo on client_b")
            .expect("stream ended")
            .expect("ws error");
        assert_eq!(reply, tungstenite::Message::Text("still-here".into()));

        command_tx.send(ManagerCommand::Shutdown).await.unwrap();
        let _ = timeout(Duration::from_secs(2), manager_handle).await;
    }

    // -------------------------------------------------------------------------
    // Test: handshake failure does not stop the server (Requirement 1.4)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn handshake_failure_does_not_stop_server() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let manager: ConnectionManager<tokio::net::TcpStream> = ConnectionManager::new();
        let event_tx = manager.event_tx();
        let command_tx = manager.command_tx();
        let manager_handle = tokio::spawn(manager.run());
        let (listener_cmd_tx, listener_cmd_rx) = mpsc::channel::<ManagerCommand>(1);
        tokio::spawn(run_tcp_listener_inner(listener, event_tx.clone(), listener_cmd_rx));

        // Send garbage — handshake should fail
        let mut plain = tokio::net::TcpStream::connect(addr).await.unwrap();
        plain.write_all(b"NOT A WEBSOCKET\r\n\r\n").await.unwrap();
        drop(plain);
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Valid WS connection should still work
        let url = format!("ws://{}", addr);
        let (_ws, _) = tokio_tungstenite::connect_async(&url).await
            .expect("valid WS connection should succeed after handshake failure");
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Shut down — signal both listener and manager
        listener_cmd_tx.send(ManagerCommand::Shutdown).await.unwrap();
        command_tx.send(ManagerCommand::Shutdown).await.unwrap();
        let result = timeout(Duration::from_secs(2), manager_handle).await;
        assert!(result.is_ok(), "manager should shut down within 2 seconds");
    }

    // =========================================================================
    // Property-based tests
    // =========================================================================

    // Helper: run an async block inside a proptest case using a single-threaded runtime.
    // Returns Err(TestCaseError) if the async block returns one.
    macro_rules! prop_async {
        ($body:expr) => {{
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move { $body })
        }};
    }

    // -------------------------------------------------------------------------
    // Property 1: Connection registration (Req 1.2, 1.3)
    // Feature: go-fish-game-server, Property 1: Connection registration
    // -------------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_connection_registration(
            a in 1u8..=254u8,
            b in 0u8..=255u8,
            c in 0u8..=255u8,
            d in 1u8..=254u8,
            port in 1024u16..=49151u16,
            msg in "[a-zA-Z0-9]{1,32}",
        ) {
            prop_async!({
                let addr: SocketAddr = format!("{a}.{b}.{c}.{d}:{port}").parse().unwrap();
                let (event_tx, command_tx, manager_handle) = start_manager();

                let mut client_ws = connect_client(&event_tx, addr).await;
                tokio::time::sleep(Duration::from_millis(10)).await;

                client_ws.send(tungstenite::Message::Text(msg.clone().into())).await.unwrap();

                let reply = timeout(Duration::from_secs(2), client_ws.next()).await;
                command_tx.send(ManagerCommand::Shutdown).await.unwrap();
                let _ = timeout(Duration::from_secs(2), manager_handle).await;

                match reply {
                    Ok(Some(Ok(tungstenite::Message::Text(t)))) => {
                        prop_assert_eq!(t.as_str(), msg.as_str());
                    }
                    _ => return Err(TestCaseError::fail("client was not registered — no echo received")),
                }
                Ok(())
            }).unwrap();
        }
    }

    // -------------------------------------------------------------------------
    // Property 2: Echo round-trip (Req 3.1, 3.2)
    // Feature: go-fish-game-server, Property 2: Echo round-trip
    // -------------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_echo_round_trip(msg in "[a-zA-Z0-9 ]{1,64}") {
            prop_async!({
                let addr: SocketAddr = "127.0.0.1:20001".parse().unwrap();
                let (event_tx, command_tx, manager_handle) = start_manager();

                let mut client_ws = connect_client(&event_tx, addr).await;
                tokio::time::sleep(Duration::from_millis(10)).await;

                client_ws.send(tungstenite::Message::Text(msg.clone().into())).await.unwrap();

                let reply = timeout(Duration::from_secs(2), client_ws.next()).await;
                command_tx.send(ManagerCommand::Shutdown).await.unwrap();
                let _ = timeout(Duration::from_secs(2), manager_handle).await;

                match reply {
                    Ok(Some(Ok(tungstenite::Message::Text(t)))) => {
                        prop_assert_eq!(t.as_str(), msg.as_str(), "echo must preserve exact text");
                    }
                    _ => return Err(TestCaseError::fail("did not receive echo")),
                }
                Ok(())
            }).unwrap();
        }
    }

    // -------------------------------------------------------------------------
    // Property 3: Echo isolation (Req 3.3, 5.2)
    // Feature: go-fish-game-server, Property 3: Echo isolation
    // -------------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_echo_isolation(
            n_clients in 2usize..=5usize,
            sender_idx in 0usize..5usize,
            msg in "[a-zA-Z0-9]{1,32}",
        ) {
            prop_async!({
                let (event_tx, command_tx, manager_handle) = start_manager();
                let sender_idx = sender_idx % n_clients;

                let mut clients: Vec<(SocketAddr, WebSocketStream<tokio::io::DuplexStream>)> = Vec::new();
                for i in 0..n_clients {
                    let addr: SocketAddr = format!("127.0.0.1:{}", 21000 + i as u16).parse().unwrap();
                    let ws = connect_client(&event_tx, addr).await;
                    clients.push((addr, ws));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;

                clients[sender_idx].1
                    .send(tungstenite::Message::Text(msg.clone().into()))
                    .await
                    .unwrap();

                // Sender should receive the echo
                let reply = timeout(Duration::from_secs(2), clients[sender_idx].1.next()).await;
                match reply {
                    Ok(Some(Ok(tungstenite::Message::Text(t)))) => {
                        prop_assert_eq!(t.as_str(), msg.as_str());
                    }
                    _ => return Err(TestCaseError::fail("sender did not receive echo")),
                }

                // All other clients should receive nothing
                for (i, (_, ws)) in clients.iter_mut().enumerate() {
                    if i == sender_idx { continue; }
                    let nothing = timeout(Duration::from_millis(50), ws.next()).await;
                    if nothing.is_ok() {
                        return Err(TestCaseError::fail(
                            format!("client {i} received a message it should not have")
                        ));
                    }
                }

                command_tx.send(ManagerCommand::Shutdown).await.unwrap();
                let _ = timeout(Duration::from_secs(2), manager_handle).await;
                Ok(())
            }).unwrap();
        }
    }

    // -------------------------------------------------------------------------
    // Property 4: Disconnection removes client (Req 4.1, 4.2)
    // Feature: go-fish-game-server, Property 4: Disconnection removes client
    // -------------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_disconnection_removes_client(msg in "[a-zA-Z0-9]{1,32}") {
            prop_async!({
                let addr: SocketAddr = "127.0.0.1:22001".parse().unwrap();
                let (event_tx, command_tx, manager_handle) = start_manager();

                let mut client_ws = connect_client(&event_tx, addr).await;
                tokio::time::sleep(Duration::from_millis(10)).await;

                // Disconnect via Close frame
                client_ws.send(tungstenite::Message::Close(None)).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;

                // Send a message to the now-disconnected address — should be a no-op
                event_tx.send(ClientEvent::Message { address: addr, text: msg.clone() }).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;

                // The client stream should be closed, not receive the message
                let next = timeout(Duration::from_millis(100), client_ws.next()).await;
                match next {
                    Ok(Some(Ok(tungstenite::Message::Text(t)))) => {
                        return Err(TestCaseError::fail(
                            format!("disconnected client received unexpected message: {t}")
                        ));
                    }
                    _ => {} // closed, error, or timeout — all acceptable
                }

                command_tx.send(ManagerCommand::Shutdown).await.unwrap();
                let _ = timeout(Duration::from_secs(2), manager_handle).await;
                Ok(())
            }).unwrap();
        }
    }

    // -------------------------------------------------------------------------
    // Property 5: Disconnect does not affect remaining clients (Req 4.4)
    // Feature: go-fish-game-server, Property 5: Disconnect does not affect remaining clients
    // -------------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]
        #[test]
        fn prop_disconnect_does_not_affect_remaining(msg in "[a-zA-Z0-9]{1,32}") {
            prop_async!({
                let addr_a: SocketAddr = "127.0.0.1:23001".parse().unwrap();
                let addr_b: SocketAddr = "127.0.0.1:23002".parse().unwrap();
                let (event_tx, command_tx, manager_handle) = start_manager();

                let mut client_a = connect_client(&event_tx, addr_a).await;
                let mut client_b = connect_client(&event_tx, addr_b).await;
                tokio::time::sleep(Duration::from_millis(10)).await;

                // Disconnect client_a
                client_a.send(tungstenite::Message::Close(None)).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;

                // client_b should still echo correctly
                client_b.send(tungstenite::Message::Text(msg.clone().into())).await.unwrap();
                let reply = timeout(Duration::from_secs(2), client_b.next()).await;
                match reply {
                    Ok(Some(Ok(tungstenite::Message::Text(t)))) => {
                        prop_assert_eq!(t.as_str(), msg.as_str());
                    }
                    _ => return Err(TestCaseError::fail("remaining client did not receive echo after disconnect")),
                }

                command_tx.send(ManagerCommand::Shutdown).await.unwrap();
                let _ = timeout(Duration::from_secs(2), manager_handle).await;
                Ok(())
            }).unwrap();
        }
    }

}
