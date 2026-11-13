//! Build benchmarks: load and render a generated site in memory at three sizes.

use assert_fs::TempDir;
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use reuben_core::site::{RenderOptions, Site};
use reuben_core::testsite;

fn build(c: &mut Criterion) {
    let mut group = c.benchmark_group("build");
    group.sample_size(10);
    for pages in [100usize, 1_000, 5_000] {
        let tmp = TempDir::new().unwrap();
        testsite::generate(tmp.path(), pages).unwrap();
        let render = || {
            let site = Site::load_with(tmp.path(), &Default::default()).unwrap();
            let (_, report) = site
                .render(&RenderOptions {
                    drafts: false,
                    jobs: None,
                })
                .unwrap();
            assert_eq!(report.pages_written, pages);
            report
        };
        eprintln!(
            "{pages} pages: {} accessibility findings",
            render().findings.len()
        );
        group.throughput(Throughput::Elements(pages as u64));
        group.bench_function(format!("{pages}_pages"), |b| b.iter(render));
    }
    group.finish();
}

criterion_group!(benches, build);
criterion_main!(benches);
