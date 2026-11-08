//! Build benchmarks: load and render a generated site in memory at three sizes.

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use reuben_core::site::{RenderOptions, Site};
use reuben_core::testsite;

fn build(c: &mut Criterion) {
    let mut group = c.benchmark_group("build");
    group.sample_size(10);
    for pages in [100usize, 1_000, 5_000] {
        let tmp = tempdir(pages);
        testsite::generate(&tmp, pages).unwrap();
        group.throughput(Throughput::Elements(pages as u64));
        group.bench_function(format!("{pages}_pages"), |b| {
            b.iter_batched(
                || (),
                |()| {
                    let site = Site::load_with(&tmp, &Default::default()).unwrap();
                    let (_, report) = site
                        .render(&RenderOptions {
                            drafts: false,
                            jobs: None,
                        })
                        .unwrap();
                    assert_eq!(report.pages_written, pages);
                },
                BatchSize::PerIteration,
            );
        });
        std::fs::remove_dir_all(&tmp).unwrap();
    }
    group.finish();
}

fn tempdir(pages: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("reuben-bench-{}-{pages}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

criterion_group!(benches, build);
criterion_main!(benches);
