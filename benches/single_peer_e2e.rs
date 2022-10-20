extern crate reed_solomon_erasure;

use criterion::{criterion_group, criterion_main, BatchSize, Throughput};

use reed_solomon_erasure::galois_8::ReedSolomon;
// or use the following for Galois 2^16 backend
// use reed_solomon_erasure::galois_16::ReedSolomon;
use criterion::async_executor::AsyncStdExecutor;
use criterion::BenchmarkId;
use criterion::Criterion;
use futures::{pin_mut, prelude::*};
use graphsync::{GraphSync, GraphSyncEvent, Request};
use ipld_traversal::{
    blockstore::MemoryBlockstore,
    selector::{RecursionLimit, Selector},
    LinkSystem, Prefix,
};
use libipld::{ipld, Cid, Ipld};
use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::transport::Boxed;
use libp2p::noise::{Keypair, NoiseConfig, X25519Spec};
use libp2p::swarm::{Swarm, SwarmEvent};
use libp2p::tcp::{GenTcpConfig, TcpTransport};
use libp2p::{identity, mplex};
use libp2p::{PeerId, Transport};
use rand::prelude::*;
use std::time::Duration;

fn mk_transport() -> (PeerId, Boxed<(PeerId, StreamMuxerBox)>) {
    let id_key = identity::Keypair::generate_ed25519();
    let peer_id = id_key.public().to_peer_id();
    let dh_key = Keypair::<X25519Spec>::new()
        .into_authentic(&id_key)
        .unwrap();
    let noise = NoiseConfig::xx(dh_key).into_authenticated();

    let transport = TcpTransport::new(GenTcpConfig::new().nodelay(true))
        .upgrade(libp2p::core::upgrade::Version::V1)
        .authenticate(noise)
        .multiplex(mplex::MplexConfig::new())
        .timeout(Duration::from_secs(20))
        .boxed();
    (peer_id, transport)
}

fn create_shards(
    shard_size: usize,
    num_data: usize,
    num_parity: usize,
) -> (MemoryBlockstore, Cid, ReedSolomon) {
    assert!(shard_size > 0);
    let store = MemoryBlockstore::new();
    let lsys = LinkSystem::new(store.clone());
    let mut links = Vec::new();

    let r = ReedSolomon::new(num_data, num_parity).unwrap(); // 3 data shards, 2 parity shards

    let mut shards = vec![vec![0u8; shard_size]; num_data + num_parity];
    // leave parity shards as 0 data
    let _ = shards[0..num_data]
        .iter_mut()
        .map(|s| rand::thread_rng().fill_bytes(s))
        .collect::<Vec<_>>();

    // Construct the parity shards
    r.encode(&mut shards).unwrap();

    for s in shards.into_iter() {
        // each entry is 8-bit so the len is the number of bytes
        let size = s.len();
        if size == 0 {
            break;
        }

        let cid = lsys
            .store(Prefix::new(0x55, 0x12), &Ipld::Bytes(s.clone()))
            .expect("link system should store shard");
        links.push(ipld!({
            "Hash": cid,
            "Tsize": size,
        }));
    }
    let root_node = ipld!({
        "Links": links,
    });
    let root = lsys
        .store(Prefix::new(0x71, 0x12), &root_node)
        .expect("link system to store root node");
    (store, root, r)
}

async fn run_local_transfer(store: MemoryBlockstore, root: Cid) {
    let (peer1, trans) = mk_transport();
    let mut swarm1 = Swarm::new(trans, GraphSync::new(store), peer1);

    Swarm::listen_on(&mut swarm1, "/ip4/127.0.0.1/tcp/0".parse().unwrap()).unwrap();

    let listener_addr = async {
        loop {
            let swarm1_fut = swarm1.select_next_some();
            pin_mut!(swarm1_fut);
            match swarm1_fut.await {
                SwarmEvent::NewListenAddr { address, .. } => return address,
                _ => {}
            }
        }
    }
    .await;

    let (peer2, trans) = mk_transport();
    let mut swarm2 = Swarm::new(trans, GraphSync::new(MemoryBlockstore::new()), peer2);

    let client = swarm2.behaviour_mut();
    client.add_address(&peer1, listener_addr);

    let selector = Selector::ExploreRecursive {
        limit: RecursionLimit::None,
        sequence: Box::new(Selector::ExploreAll {
            next: Box::new(Selector::ExploreRecursiveEdge),
        }),
        current: None,
    };

    let req = Request::builder()
        .root(root)
        .selector(selector)
        .build()
        .unwrap();
    client.request(peer1, req.clone());

    loop {
        let swarm1_fut = swarm1.select_next_some();
        let swarm2_fut = swarm2.select_next_some();
        pin_mut!(swarm1_fut);
        pin_mut!(swarm2_fut);

        match future::select(swarm1_fut, swarm2_fut)
            .await
            .factor_second()
            .0
        {
            future::Either::Right(SwarmEvent::Behaviour(GraphSyncEvent::Completed { .. })) => {
                return;
            }
            _ => continue,
        }
    }
}

fn bench_ec_graphsync(c: &mut Criterion) {
    static KB: usize = 1024;

    let mut group = c.benchmark_group("ec-graphsync");

    // for shard_size in [KB, 4 * KB, 15 * KB, 60 * KB].iter() {
    //     group.throughput(Throughput::Bytes(*shard_size as u64));
    //     group.bench_with_input(
    //         BenchmarkId::new("varying shard size", shard_size),
    //         shard_size,
    //         move |b, &shard_size| {
    //             b.to_async(AsyncStdExecutor).iter_batched(
    //                 || create_shards(shard_size, 10, 2),
    //                 |(store, root, _)| async move { run_local_transfer(store, root).await },
    //                 BatchSize::SmallInput,
    //             );
    //         },
    //     );
    // }

    for num_data in [10, 100, 250].iter() {
        group.throughput(Throughput::Bytes(*num_data as u64));
        group.bench_with_input(
            BenchmarkId::new("varying number of data size", num_data),
            num_data,
            move |b, &num_data| {
                b.to_async(AsyncStdExecutor).iter_batched(
                    || create_shards(KB, num_data, num_data),
                    |(store, root, _)| async move { run_local_transfer(store, root).await },
                    BatchSize::SmallInput,
                );
            },
        );
    }
}

criterion_group!(benches, bench_ec_graphsync);
criterion_main!(benches);
