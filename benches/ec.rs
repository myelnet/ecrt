extern crate reed_solomon_erasure;

use reed_solomon_erasure::galois_8::ReedSolomon;
// or use the following for Galois 2^16 backend
// use reed_solomon_erasure::galois_16::ReedSolomon;
use criterion::BenchmarkId;
use criterion::Criterion;
use criterion::{criterion_group, criterion_main, BatchSize, Throughput};
use rand::prelude::*;
use rand::Rng;

fn create_shards(
    shard_size: usize,
    num_data: usize,
    num_parity: usize,
) -> (ReedSolomon, Vec<Vec<u8>>) {
    assert!(shard_size > 0 && num_data > 0);

    let r = ReedSolomon::new(num_data, num_parity).unwrap();

    let mut shards = vec![vec![0u8; shard_size]; num_data + num_parity];
    // leave parity shards as 0 data
    let _ = shards[0..num_data]
        .iter_mut()
        .map(|s| rand::thread_rng().fill_bytes(s))
        .collect::<Vec<_>>();

    // Construct the parity shards
    r.encode(&mut shards).unwrap();

    (r, shards)
}

fn decode_shards(r: ReedSolomon, shards: Vec<Vec<u8>>, num_lost: usize) {
    // Make a copy and transform it into option shards arrangement
    // for feeding into reconstruct_shards
    let mut shards: Vec<_> = shards.iter().cloned().map(Some).collect();

    let _ = (0..num_lost)
        .map(|_| {
            let idx = rand::thread_rng().gen_range(0..shards.len());
            shards[idx] = None
        })
        .collect::<Vec<_>>();

    // Try to reconstruct missing shards
    r.reconstruct(&mut shards).unwrap();

    // Convert back to normal shard arrangement
    let result: Vec<_> = shards.into_iter().filter_map(|x| x).collect();

    assert!(r.verify(&result).unwrap());
}

fn bench_encoding(c: &mut Criterion) {
    static MB: usize = 1024 * 1024;

    let mut group = c.benchmark_group("encoding");

    for shard_size in [MB, 4 * MB, 15 * MB, 60 * MB].iter() {
        group.throughput(Throughput::Bytes(*shard_size as u64));
        group.bench_with_input(
            BenchmarkId::new("varying shard size", shard_size),
            shard_size,
            |b, &_| {
                b.iter(|| {
                    create_shards(*shard_size, 10, 2);
                });
            },
        );
    }

    for num_data in [10, 100, 250].iter() {
        group.throughput(Throughput::Bytes(*num_data as u64));
        group.bench_with_input(
            BenchmarkId::new("varying number of data shards", num_data),
            num_data,
            |b, &_| {
                b.iter(|| {
                    create_shards(MB, *num_data, 2);
                });
            },
        );
    }
}

fn bench_decoding(c: &mut Criterion) {
    static MB: usize = 1024 * 1024;

    let mut group = c.benchmark_group("decoding");

    for shard_size in [MB, 4 * MB, 15 * MB, 60 * MB].iter() {
        group.throughput(Throughput::Bytes(*shard_size as u64));
        group.bench_with_input(
            BenchmarkId::new("varying shard size", shard_size),
            shard_size,
            |b, &_| {
                b.iter_batched(
                    || create_shards(*shard_size, 10, 10),
                    |(r, shards)| {
                        decode_shards(r, shards, 5);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    for num_data in [10, 100, 250].iter() {
        group.throughput(Throughput::Bytes(*num_data as u64));
        group.bench_with_input(
            BenchmarkId::new("varying number of data shards", num_data),
            num_data,
            |b, &_| {
                b.iter_batched(
                    || create_shards(MB, *num_data, *num_data),
                    |(r, shards)| {
                        decode_shards(r, shards, 5);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    for num_lost in [10, 100, 250].iter() {
        group.throughput(Throughput::Bytes(*num_lost as u64));
        group.bench_with_input(
            BenchmarkId::new("varying error rate", num_lost),
            num_lost,
            |b, &_| {
                b.iter_batched(
                    || create_shards(MB, 250, 250),
                    |(r, shards)| {
                        decode_shards(r, shards, *num_lost);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
}

criterion_group!(benches, bench_encoding, bench_decoding);
criterion_main!(benches);
