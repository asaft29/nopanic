//! Criterion benchmarks for circuit building performance.
//!
//! Measures circuit build time using relay simulators chained to
//! form 3-, 5-, and 10-hop circuits.

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::expect_used
)]

use common::crypto::CipherPair;
use common::{
    Message, MessageCommand, NodeDescriptor, NodeType, PublicKey, RelayStream, RelayTlsConfig,
    server_name_from_addr,
};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rand_core::OsRng;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::runtime::Runtime;
use tor_client::core::circuit::CircuitBuilder;
use tor_client::core::transport::TcpTlsTransport;
use tor_llcrypto::pk::curve25519::{PublicKey as X25519PublicKey, StaticSecret};

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

async fn handle_relay_session(
    stream: &mut RelayStream,
    identity: &RelayIdentity,
    downstream_addr: Option<std::net::SocketAddr>,
    downstream_fp: Option<String>,
) {
    let create_msg = Message::from_stream(stream).await.unwrap().unwrap();
    assert_eq!(create_msg.command, MessageCommand::Create);
    let mut client_pub = [0u8; 32];
    client_pub.copy_from_slice(&create_msg.data[0..32]);
    let (created_payload, session_key) = relay_ntor(&client_pub, identity);
    let mut cipher = CipherPair::new(&session_key);
    stream
        .write_all(&Message::created(create_msg.circuit_id, created_payload.to_vec()).to_bytes())
        .await
        .unwrap();

    if downstream_addr.is_none() {
        return;
    }

    let ds_addr = downstream_addr.unwrap();
    let ds_fp = downstream_fp.unwrap();

    let ds_tcp = tokio::net::TcpStream::connect(ds_addr).await.unwrap();
    let connector = RelayTlsConfig::make_tls_connector(&ds_fp).unwrap();
    let server_name = server_name_from_addr(ds_addr);
    let ds_tls = connector.connect(server_name, ds_tcp).await.unwrap();
    let mut downstream: RelayStream = Box::new(ds_tls);

    let cid = create_msg.circuit_id;
    let mut first_extend = true;

    loop {
        let msg = match Message::from_stream(stream).await {
            Ok(Some(m)) => m,
            _ => break,
        };
        assert_eq!(msg.circuit_id, cid);

        let mut inner = msg.data.clone();
        cipher.apply_forward(&mut inner);

        if first_extend {
            first_extend = false;
            let null_pos = inner.iter().position(|&b| b == 0).unwrap();
            let key_start = null_pos + 1;
            let inner_payload = &inner[key_start..key_start + 32];

            let fwd_create = Message::create(msg.circuit_id, inner_payload.to_vec());
            fwd_create.write_to_stream(&mut downstream).await.unwrap();

            let created = Message::from_stream(&mut downstream)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(created.command, MessageCommand::Created);

            let mut encrypted = created.data.clone();
            cipher.apply_backward(&mut encrypted);
            stream
                .write_all(&Message::extended(msg.circuit_id, encrypted).to_bytes())
                .await
                .unwrap();
        } else {
            let fwd_extend = Message::extend(msg.circuit_id, inner);
            fwd_extend.write_to_stream(&mut downstream).await.unwrap();

            let extended = Message::from_stream(&mut downstream)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(extended.command, MessageCommand::Extended);

            let mut encrypted = extended.data.clone();
            cipher.apply_backward(&mut encrypted);
            stream
                .write_all(&Message::extended(msg.circuit_id, encrypted).to_bytes())
                .await
                .unwrap();
        }
    }
}

fn spawn_generic_relay(
    identity: RelayIdentity,
    tls_acceptor: Arc<dyn common::tls::StreamAcceptor>,
    downstream_addr: Option<std::net::SocketAddr>,
    downstream_fp: Option<String>,
) -> std::net::SocketAddr {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind relay listener");
    listener
        .set_nonblocking(true)
        .expect("Failed to set nonblocking");
    let listener =
        tokio::net::TcpListener::from_std(listener).expect("Failed to create tokio listener");
    let addr = listener.local_addr().unwrap();

    let id = Arc::new(identity);
    let tls = tls_acceptor;

    tokio::spawn(async move {
        loop {
            let (tcp_stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let stream = match tls.accept(tcp_stream).await {
                Ok(s) => s,
                Err(_) => continue,
            };
            let id = id.clone();
            let addr = downstream_addr;
            let fp = downstream_fp.clone();
            tokio::spawn(async move {
                let mut stream = stream;
                handle_relay_session(&mut stream, &id, addr, fp).await;
            });
        }
    });

    addr
}

async fn build_relay_chain(
    hops: u32,
) -> (Vec<NodeDescriptor>, Vec<RelayTlsConfig>, Vec<RelayIdentity>) {
    assert!(hops >= 3);
    let middle_count = hops - 2;

    let identities: Vec<RelayIdentity> = (0..hops).map(|_| RelayIdentity::generate()).collect();

    let tls_configs: Vec<RelayTlsConfig> = (0..hops)
        .map(|i| {
            let label = format!("relay{}-bench", i);
            RelayTlsConfig::generate(&label, "127.0.0.1:0".parse().unwrap()).unwrap()
        })
        .collect();

    let entries: Vec<_> = identities
        .iter()
        .map(|id| PublicKey::new(id.public_bytes))
        .collect();

    let exit_index = hops as usize - 1;

    // Spawn relays in reverse: exit → middles → entry
    let exit_addr = spawn_generic_relay(
        identities[exit_index].clone(),
        tls_configs[exit_index].acceptor.clone(),
        None,
        None,
    );

    let mut prev_addr = exit_addr;
    let mut prev_fp = tls_configs[exit_index].fingerprint.clone();

    for i in (1..=middle_count).rev() {
        let idx = i as usize;
        let addr = spawn_generic_relay(
            identities[idx].clone(),
            tls_configs[idx].acceptor.clone(),
            Some(prev_addr),
            Some(prev_fp),
        );
        prev_addr = addr;
        prev_fp = tls_configs[idx].fingerprint.clone();
    }

    // Entry (index 0)
    let entry_addr = spawn_generic_relay(
        identities[0].clone(),
        tls_configs[0].acceptor.clone(),
        Some(prev_addr),
        Some(prev_fp),
    );

    let mut nodes = Vec::with_capacity(hops as usize);
    nodes.push(node_at(
        "entry",
        NodeType::Entry,
        entry_addr,
        entries[0].clone(),
        tls_configs[0].fingerprint.clone(),
    ));
    for i in 1..=middle_count {
        let idx = i as usize;
        let label = format!("middle{}", i);
        nodes.push(node_at(
            &label,
            NodeType::Middle,
            prev_addr,
            entries[idx].clone(),
            tls_configs[idx].fingerprint.clone(),
        ));
    }
    nodes.push(node_at(
        "exit",
        NodeType::Exit,
        exit_addr,
        entries[exit_index].clone(),
        tls_configs[exit_index].fingerprint.clone(),
    ));

    (nodes, tls_configs, identities)
}

fn bench_circuit_build(c: &mut Criterion) {
    let transport = TcpTlsTransport;
    let handshaker = common::crypto::TorNtorHandshaker;

    for hops in [3u32, 5, 10] {
        let rt = Arc::new(Runtime::new().unwrap());
        let (path, _tls, _ids) = rt.block_on(build_relay_chain(hops));
        let path = Arc::new(path);
        let circuit_counter = AtomicU32::new(1);

        let mut group = c.benchmark_group(format!("circuit_build/{}hop", hops));
        group.throughput(Throughput::Elements(1));
        group.sample_size(if hops == 10 { 50 } else { 100 });

        let t = &transport as &dyn tor_client::core::transport::TransportLayer;
        let h = &handshaker as &dyn common::crypto::NtorHandshaker;
        let rt = Arc::clone(&rt);

        group.bench_function(format!("{}_hop", hops), move |b| {
            let path = Arc::clone(&path);
            b.iter(|| {
                let cid = circuit_counter.fetch_add(1, Ordering::SeqCst);
                rt.block_on(async { CircuitBuilder::build(cid, &path, t, h).await.unwrap() })
            })
        });

        group.finish();
    }
}

fn bench_concurrency(c: &mut Criterion) {
    let rt = Arc::new(Runtime::new().unwrap());
    let (path, _tls, _ids) = rt.block_on(build_relay_chain(3));
    let path = Arc::new(path);

    for clients in [1u32, 2, 4, 8, 16] {
        let mut group = c.benchmark_group(format!("concurrency/{}clients", clients));
        group.throughput(Throughput::Elements(clients as u64));
        group.sample_size(30);

        let path = Arc::clone(&path);
        let rt = Arc::clone(&rt);

        group.bench_function(format!("{}_clients", clients), move |b| {
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::from_secs(0);
                for _ in 0..iters {
                    let path = Arc::clone(&path);
                    let start = std::time::Instant::now();
                    rt.block_on(async {
                        let mut handles = Vec::with_capacity(clients as usize);
                        for i in 0..clients {
                            let p = Arc::clone(&path);
                            handles.push(tokio::task::spawn(async move {
                                let transport = TcpTlsTransport;
                                let handshaker = common::crypto::TorNtorHandshaker;
                                CircuitBuilder::build(i + 1, &p, &transport, &handshaker)
                                    .await
                                    .unwrap()
                            }));
                        }
                        for h in handles {
                            h.await.unwrap();
                        }
                    });
                    total += start.elapsed();
                }
                total
            })
        });

        group.finish();
    }
}

criterion_group!(benches, bench_circuit_build, bench_concurrency);
criterion_main!(benches);
