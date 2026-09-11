use std::collections::HashMap;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use kube::runtime::watcher::Event;
use terrarium::k8s::terraform::Terraform;
use terrarium::k8s::watcher::{TfStore, create_tf_store};
use terrarium::state::store::SortColumn;
use terrarium::ui::resource_list::get_filtered_terraforms;

fn terraform(index: usize) -> Terraform {
    let (ready, reason) = match index % 3 {
        0 => ("True", "ReconciliationSucceeded"),
        1 => ("False", "TerraformPlanFailed"),
        _ => ("Unknown", "Progressing"),
    };

    serde_json::from_value(serde_json::json!({
        "apiVersion": "infra.contrib.fluxcd.io/v1alpha2",
        "kind": "Terraform",
        "metadata": {
            "name": format!("terraform-{index:05}"),
            "namespace": format!("namespace-{:02}", index % 20),
            "uid": format!("uid-{index}"),
            "resourceVersion": index.to_string(),
            "creationTimestamp": "2026-01-01T00:00:00Z"
        },
        "spec": {
            "interval": "1m",
            "sourceRef": {"kind": "GitRepository", "name": "source"}
        },
        "status": {
            "lastAppliedRevision": format!("main@sha1:{index:040x}"),
            "conditions": [{
                "type": "Ready",
                "status": ready,
                "reason": reason,
                "message": "benchmark fixture",
                "lastTransitionTime": "2026-08-24T12:00:00Z"
            }]
        }
    }))
    .expect("benchmark Terraform fixture should deserialize")
}

fn terraform_store(count: usize) -> TfStore {
    let (store, mut writer) = create_tf_store();
    for index in 0..count {
        writer.apply_watcher_event(&Event::Apply(terraform(index)));
    }
    store
}

fn benchmark_terraform_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("terraform_filter_sort");
    let recently_acted = HashMap::new();

    for count in [500, 2_000] {
        let store = terraform_store(count);
        group.throughput(Throughput::Elements(count as u64));

        group.bench_with_input(
            BenchmarkId::new("sort_by_name", count),
            &store,
            |b, store| {
                b.iter(|| {
                    black_box(get_filtered_terraforms(
                        store,
                        &None,
                        "",
                        false,
                        false,
                        false,
                        false,
                        false,
                        &recently_acted,
                        SortColumn::Name,
                        false,
                    ))
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("failures_by_ready", count),
            &store,
            |b, store| {
                b.iter(|| {
                    black_box(get_filtered_terraforms(
                        store,
                        &None,
                        "",
                        true,
                        false,
                        false,
                        false,
                        false,
                        &recently_acted,
                        SortColumn::Ready,
                        false,
                    ))
                });
            },
        );

        group.bench_with_input(BenchmarkId::new("search", count), &store, |b, store| {
            b.iter(|| {
                black_box(get_filtered_terraforms(
                    store,
                    &None,
                    "terraform-001",
                    false,
                    false,
                    false,
                    false,
                    false,
                    &recently_acted,
                    SortColumn::Name,
                    false,
                ))
            });
        });
    }

    group.finish();
}

criterion_group!(benches, benchmark_terraform_filtering);
criterion_main!(benches);
