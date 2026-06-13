//! Concurrent client integration tests for the Tor circuit builder.
//!
//! These tests simulate multiple tor-clients simultaneously building circuits
//! through shared relay nodes, validating that the relay handlers correctly
//! multiplex between independent circuits without interference or race conditions.

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::doc_lazy_continuation
)]

use common::crypto::CipherPair;
use common::{
    Message, MessageCommand, NodeDescriptor, NodeType, PublicKey, RelayStream, RelayTlsConfig,
    server_name_from_addr,
};
use rand_core::OsRng;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tor_client::core::circuit::{CircuitBuilder, CircuitState};
use tor_client::core::transport::TcpTlsTransport;
use tor_llcrypto::pk::curve25519::{PublicKey as X25519PublicKey, StaticSecret};

// ═══════════════════════════════════════════════════════════════════
//  Shared helpers
// ═══════════════════════════════════════════════════════════════════

#[derive(Clone)]
struct RelayIdentity {
    secret_bytes: [u8; 32],
    public_bytes: [u8; 32],
}

impl RelayIdentity {
    fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = X25519PublicKey::from(&secret);
        Self {
            secret_bytes: secret.to_bytes(),
            public_bytes: *public.as_bytes(),
        }
    }
}

fn relay_ntor(
    client_ephemeral_pub: &[u8; 32],
    identity: &RelayIdentity,
) -> ([u8; 64], common::crypto::SessionKey) {
    let (server_eph_pub, auth, session_key) = common::crypto::ntor_server(
        &identity.secret_bytes,
        &identity.public_bytes,
        client_ephemeral_pub,
    );
    let mut payload = [0u8; 64];
    payload[..32].copy_from_slice(&server_eph_pub);
    payload[32..].copy_from_slice(&auth);
    (payload, session_key)
}

async fn accept_tls(
    listener: &TcpListener,
    tls_acceptor: &Arc<dyn common::tls::StreamAcceptor>,
) -> (RelayStream, std::net::SocketAddr) {
    let (tcp_stream, addr) = listener.accept().await.unwrap();
    let stream: RelayStream = tls_acceptor.accept(tcp_stream).await.unwrap();
    (stream, addr)
}

fn node_at(
    id: &str,
    node_type: NodeType,
    addr: std::net::SocketAddr,
    public_key: PublicKey,
    tls_cert_fingerprint: String,
) -> NodeDescriptor {
    let mut desc =
        NodeDescriptor::new(id.to_string(), node_type, addr, public_key, 1_000_000, None);
    desc.tls_cert_fingerprint = tls_cert_fingerprint;
    desc
}

// ═══════════════════════════════════════════════════════════════════
//  Multi-connection relay simulators
// ═══════════════════════════════════════════════════════════════════

async fn spawn_exit_multi(
    identity: RelayIdentity,
    tls_acceptor: Arc<dyn common::tls::StreamAcceptor>,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let id = Arc::new(identity);

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = accept_tls(&listener, &tls_acceptor).await;
            let id = id.clone();

            tokio::spawn(async move {
                let create_msg = Message::from_stream(&mut stream).await.unwrap().unwrap();
                assert_eq!(create_msg.command, MessageCommand::Create);

                let mut client_pub = [0u8; 32];
                client_pub.copy_from_slice(&create_msg.data[0..32]);
                let (created_payload, _session_key) = relay_ntor(&client_pub, &id);

                let created_msg = Message::created(create_msg.circuit_id, created_payload.to_vec());
                stream.write_all(&created_msg.to_bytes()).await.unwrap();
            });
        }
    });

    addr
}

async fn spawn_middle_multi(
    identity: RelayIdentity,
    tls_acceptor: Arc<dyn common::tls::StreamAcceptor>,
    exit_addr: std::net::SocketAddr,
    exit_fingerprint: String,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let id = Arc::new(identity);

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = accept_tls(&listener, &tls_acceptor).await;
            let id = id.clone();
            let ex_fp = exit_fingerprint.clone();

            tokio::spawn(async move {
                let create_msg = Message::from_stream(&mut stream).await.unwrap().unwrap();
                assert_eq!(create_msg.command, MessageCommand::Create);

                let mut client_pub = [0u8; 32];
                client_pub.copy_from_slice(&create_msg.data[0..32]);
                let (created_payload, session_key) = relay_ntor(&client_pub, &id);

                let mut cipher = CipherPair::new(&session_key);
                let created_msg = Message::created(create_msg.circuit_id, created_payload.to_vec());
                stream.write_all(&created_msg.to_bytes()).await.unwrap();

                let extend_msg = Message::from_stream(&mut stream).await.unwrap().unwrap();
                assert_eq!(extend_msg.command, MessageCommand::Extend);

                let mut decrypted = extend_msg.data.clone();
                cipher.apply_forward(&mut decrypted);

                let null_pos = decrypted.iter().position(|&b| b == 0).unwrap();
                let key_start = null_pos + 1;
                let inner_payload = &decrypted[key_start..key_start + 32];

                let exit_tcp = tokio::net::TcpStream::connect(exit_addr).await.unwrap();
                let connector = RelayTlsConfig::make_tls_connector(&ex_fp).unwrap();
                let server_name = server_name_from_addr(exit_addr);
                let exit_tls = connector.connect(server_name, exit_tcp).await.unwrap();
                let mut exit_stream: RelayStream = Box::new(exit_tls);

                let forward_create = Message::create(extend_msg.circuit_id, inner_payload.to_vec());
                forward_create
                    .write_to_stream(&mut exit_stream)
                    .await
                    .unwrap();

                let created_from_exit = Message::from_stream(&mut exit_stream)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(created_from_exit.command, MessageCommand::Created);

                let mut encrypted = created_from_exit.data.clone();
                cipher.apply_backward(&mut encrypted);
                let extended_msg = Message::extended(extend_msg.circuit_id, encrypted);
                stream.write_all(&extended_msg.to_bytes()).await.unwrap();
            });
        }
    });

    addr
}

async fn spawn_entry_multi(
    identity: RelayIdentity,
    tls_acceptor: Arc<dyn common::tls::StreamAcceptor>,
    middle_addr: std::net::SocketAddr,
    _exit_addr: std::net::SocketAddr,
    middle_fingerprint: String,
    _exit_fingerprint: String,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let id = Arc::new(identity);

    tokio::spawn(async move {
        loop {
            let (mut stream, _) = accept_tls(&listener, &tls_acceptor).await;
            let id = id.clone();
            let m_fp = middle_fingerprint.clone();

            tokio::spawn(async move {
                let create_msg = Message::from_stream(&mut stream).await.unwrap().unwrap();
                assert_eq!(create_msg.command, MessageCommand::Create);

                let mut client_pub = [0u8; 32];
                client_pub.copy_from_slice(&create_msg.data[0..32]);
                let (created_payload, session_key) = relay_ntor(&client_pub, &id);

                let mut cipher = CipherPair::new(&session_key);
                let created_msg = Message::created(create_msg.circuit_id, created_payload.to_vec());
                stream.write_all(&created_msg.to_bytes()).await.unwrap();

                // First EXTEND → middle
                let extend_msg = Message::from_stream(&mut stream).await.unwrap().unwrap();
                assert_eq!(extend_msg.command, MessageCommand::Extend);

                let mut decrypted = extend_msg.data.clone();
                cipher.apply_forward(&mut decrypted);

                let null_pos = decrypted.iter().position(|&b| b == 0).unwrap();
                let key_start = null_pos + 1;
                let inner_payload = &decrypted[key_start..key_start + 32];

                let middle_tcp = tokio::net::TcpStream::connect(middle_addr).await.unwrap();
                let connector = RelayTlsConfig::make_tls_connector(&m_fp).unwrap();
                let server_name = server_name_from_addr(middle_addr);
                let middle_tls = connector.connect(server_name, middle_tcp).await.unwrap();
                let mut next_stream: RelayStream = Box::new(middle_tls);

                let forward_create = Message::create(extend_msg.circuit_id, inner_payload.to_vec());
                forward_create
                    .write_to_stream(&mut next_stream)
                    .await
                    .unwrap();

                let created_from_middle = Message::from_stream(&mut next_stream)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(created_from_middle.command, MessageCommand::Created);

                let mut encrypted = created_from_middle.data.clone();
                cipher.apply_backward(&mut encrypted);
                let extended_msg = Message::extended(extend_msg.circuit_id, encrypted);
                stream.write_all(&extended_msg.to_bytes()).await.unwrap();

                // Second EXTEND → exit
                let extend2_msg = Message::from_stream(&mut stream).await.unwrap().unwrap();
                assert_eq!(extend2_msg.command, MessageCommand::Extend);

                let mut after_entry = extend2_msg.data.clone();
                cipher.apply_forward(&mut after_entry);

                let fwd_extend = Message::extend(extend2_msg.circuit_id, after_entry);
                fwd_extend.write_to_stream(&mut next_stream).await.unwrap();

                let extended_from_middle = Message::from_stream(&mut next_stream)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(extended_from_middle.command, MessageCommand::Extended);

                let mut encrypted2 = extended_from_middle.data.clone();
                cipher.apply_backward(&mut encrypted2);
                let extended2_msg = Message::extended(extend2_msg.circuit_id, encrypted2);
                stream.write_all(&extended2_msg.to_bytes()).await.unwrap();
            });
        }
    });

    addr
}

fn make_3_hop_path(
    entry_addr: std::net::SocketAddr,
    middle_addr: std::net::SocketAddr,
    exit_addr: std::net::SocketAddr,
    entry_pub: PublicKey,
    middle_pub: PublicKey,
    exit_pub: PublicKey,
    entry_fp: String,
    middle_fp: String,
    exit_fp: String,
) -> Vec<NodeDescriptor> {
    vec![
        node_at("entry", NodeType::Entry, entry_addr, entry_pub, entry_fp),
        node_at(
            "middle",
            NodeType::Middle,
            middle_addr,
            middle_pub,
            middle_fp,
        ),
        node_at("exit", NodeType::Exit, exit_addr, exit_pub, exit_fp),
    ]
}

// ═══════════════════════════════════════════════════════════════════
//  Test 1: Multiple concurrent clients, same 3 relays
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_concurrent_clients_same_relays() {
    let entry_id = RelayIdentity::generate();
    let middle_id = RelayIdentity::generate();
    let exit_id = RelayIdentity::generate();

    let entry_pub = PublicKey::new(entry_id.public_bytes);
    let middle_pub = PublicKey::new(middle_id.public_bytes);
    let exit_pub = PublicKey::new(exit_id.public_bytes);

    let entry_tls = RelayTlsConfig::generate("entry-conc", "127.0.0.1:0".parse().unwrap()).unwrap();
    let middle_tls =
        RelayTlsConfig::generate("middle-conc", "127.0.0.1:0".parse().unwrap()).unwrap();
    let exit_tls = RelayTlsConfig::generate("exit-conc", "127.0.0.1:0".parse().unwrap()).unwrap();

    let entry_fp = entry_tls.fingerprint.clone();
    let middle_fp = middle_tls.fingerprint.clone();
    let exit_fp = exit_tls.fingerprint.clone();

    // Spawn exit first (middle needs its address)
    let exit_addr = spawn_exit_multi(exit_id, exit_tls.acceptor.clone()).await;
    let middle_addr = spawn_middle_multi(
        middle_id.clone(),
        middle_tls.acceptor.clone(),
        exit_addr,
        exit_fp.clone(),
    )
    .await;
    let entry_addr = spawn_entry_multi(
        entry_id.clone(),
        entry_tls.acceptor.clone(),
        middle_addr,
        exit_addr,
        middle_fp.clone(),
        exit_fp.clone(),
    )
    .await;

    let path = make_3_hop_path(
        entry_addr,
        middle_addr,
        exit_addr,
        entry_pub,
        middle_pub,
        exit_pub,
        entry_fp,
        middle_fp,
        exit_fp,
    );

    let num_clients = 5u32;
    let mut handles = Vec::new();
    for circuit_id in 1..=num_clients {
        let p = path.clone();
        handles.push(tokio::spawn(async move {
            let built = CircuitBuilder::build(
                circuit_id,
                &p,
                &TcpTlsTransport,
                &common::crypto::TorNtorHandshaker,
            )
            .await
            .unwrap();
            assert_eq!(built.circuit.state, CircuitState::Ready);
            assert_eq!(built.circuit.onion_keys.session_keys.len(), 3);
            built
        }));
    }

    for (i, handle) in handles.into_iter().enumerate() {
        let built = handle.await.unwrap();
        assert_eq!(built.circuit.circuit_id, (i + 1) as u32);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Test 2: Rapid circuit build + destroy (10 iterations)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_rapid_build_and_destroy() {
    let entry_id = RelayIdentity::generate();
    let middle_id = RelayIdentity::generate();
    let exit_id = RelayIdentity::generate();

    let entry_pub = PublicKey::new(entry_id.public_bytes);
    let middle_pub = PublicKey::new(middle_id.public_bytes);
    let exit_pub = PublicKey::new(exit_id.public_bytes);

    let entry_tls = RelayTlsConfig::generate("entry-d", "127.0.0.1:0".parse().unwrap()).unwrap();
    let middle_tls = RelayTlsConfig::generate("middle-d", "127.0.0.1:0".parse().unwrap()).unwrap();
    let exit_tls = RelayTlsConfig::generate("exit-d", "127.0.0.1:0".parse().unwrap()).unwrap();

    let entry_fp = entry_tls.fingerprint.clone();
    let middle_fp = middle_tls.fingerprint.clone();
    let exit_fp = exit_tls.fingerprint.clone();

    let exit_addr = spawn_exit_multi(exit_id, exit_tls.acceptor.clone()).await;
    let middle_addr = spawn_middle_multi(
        middle_id.clone(),
        middle_tls.acceptor.clone(),
        exit_addr,
        exit_fp.clone(),
    )
    .await;
    let entry_addr = spawn_entry_multi(
        entry_id.clone(),
        entry_tls.acceptor.clone(),
        middle_addr,
        exit_addr,
        middle_fp.clone(),
        exit_fp.clone(),
    )
    .await;

    let path = make_3_hop_path(
        entry_addr,
        middle_addr,
        exit_addr,
        entry_pub,
        middle_pub,
        exit_pub,
        entry_fp,
        middle_fp,
        exit_fp,
    );

    for iteration in 0..10u32 {
        let built = CircuitBuilder::build(
            iteration + 100,
            &path,
            &TcpTlsTransport,
            &common::crypto::TorNtorHandshaker,
        )
        .await
        .unwrap();

        assert_eq!(built.circuit.state, CircuitState::Ready);
        drop(built);
    }
}
