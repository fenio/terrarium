# Terrarium Improvement Backlog

This is an uncommitted engineering backlog. Items are ordered roughly by
operational risk and user impact, not by implementation size.

## P0: Fix Before More Features

- [x] Restore green CI: keep test modules after production items so `cargo clippy --all-targets -- -D warnings` passes.
- [x] Scope asynchronous results to a connection generation and request target. Delayed plan, JSON, output, event, secret, metrics, watcher, and mutation results must be discarded after a context switch or when the target view/resource changes.
- [x] Fix live-list selection identity. Track selected namespace/name/UID rather than relying only on an index; make first `j` and first Space behave consistently; reselect or clear safely after store/filter/sort updates.
- [ ] Fix reflector initial-sync state. Emit synchronized state only on `InitDone`; use separate notifications for object updates.
- [ ] Make string handling Unicode-safe. Replace byte-offset slicing in condition wrapping, viewer search highlighting, and log trimming with character-boundary/display-width-aware logic.

## P1: Correctness And Safety

- [ ] Repair Terraform plan retrieval: pass the configured workspace, match tofu-controller's long-label hashing, order chunks numerically, reject missing/duplicate chunks, and bind results to the current pending plan.
- [ ] Honor `--namespace` in Terraform, Kustomization, and GitRepository watchers instead of always using cluster-wide `Api::all()`.
- [x] Add Kubernetes mutation preconditions. Carry resource UID/resourceVersion through selection and use delete preconditions or revalidation before patches/deletes.
- [ ] Clarify deletion lifecycle. Report deletion requested until the object/finalizers are gone, and show destroy-on-delete risk prominently.
- [ ] Revisit force-unlock. It persistently writes `tfstate.forceUnlock=auto`; use a one-shot explicit lock ID or provide a clearly surfaced reset path.
- [ ] Centralize Ready/reconcile/failure/backlog classification so dashboard, filters, badges, and list views agree.
- [x] Make bulk selection scope-safe. Clear or reconcile selections when namespace/search/filter scope changes, and show target names in destructive bulk confirmations.
- [ ] Fix controller discovery for release-derived deployment names and chart label variants; make discovery errors and RBAC failures visible.
- [ ] Make metrics resilient to controller rollouts: rediscover a Ready pod, handle pod identity changes, validate HTTP status, cap response size, and consume/clean up port-forward errors.
- [ ] Bound runner log polling. Prefer on-demand logs, or use bounded concurrency, per-request deadlines, and explicit errors.
- [x] Give log streams identity and route chunks to the matching viewer; prevent old streams from contaminating preserved tabs or containers.
- [ ] Add UID-aware event filtering so recreated objects do not inherit old events.

## P1: Confidentiality And Supply Chain

- [ ] Prevent secret/output values from being sent to arbitrary browser URLs by default. If supported, require confirmation, restrict schemes/hosts, URL-encode values, limit keys, and expire sensitive caches.
- [ ] Harden config sync: reject insecure redirects, use secure exclusive temporary files, enforce size/time limits, and add signatures or pinned hashes.
- [ ] Treat `[switcher] builder` as executable trusted configuration; document the trust boundary and consider explicit opt-in.
- [ ] Fix release workflow shell interpolation. Validate strict version tags, pass values through environment variables, and use quoted heredocs or a non-shell template.
- [ ] Secure diagnostic logging with owner-only permissions, bounded rotation/redaction, and stderr redirection to a sink when file logging is unavailable.
- [ ] Make invalid configuration fail clearly instead of silently falling back to defaults or broadening invalid shortcut filters.

## P2: UX And Interaction

- [ ] Define filter capabilities per view. Unsupported filters should not appear active on Controller, Runners, Kustomizations, or custom tabs; keep Terraform drifting-only scoped to Terraform.
- [ ] Add responsive dashboard/detail layouts for small terminals, including compact/collapsible panels and one-column metrics at narrow widths.
- [ ] Make dashboard backlog scrollable and align mouse hit-testing with the rendered rows.
- [ ] Prevent header pills/gecko artwork from overwriting tab labels; reserve indicator space and truncate safely.
- [ ] Give help, shortcut popups, and status hints overflow handling or scrolling on small terminals.
- [ ] Make modal mouse input exclusive so clicks cannot affect the underlying view.
- [ ] Make viewer state per viewer instead of global: scroll, wrap, search matches, and log follow state should not leak between preserved tabs.
- [ ] Make custom tabs first-class list views: render selection markers and either implement global sorting or hide unsupported sort controls.
- [ ] Fix viewer/global navigation inconsistencies: support documented tab ranges and common help/mouse actions where safe; keep container cycling log-specific.
- [ ] Show controller logs from the actual controller pod/container list instead of assuming runner-pod state.
- [ ] Avoid trapping users in an empty shortcuts popup when all conditional entries fail to match.

## P2: Testing And Reproducibility

- [ ] Add fake-Kubernetes API integration tests for watch ordering, initial sync, namespace scope, API errors, mutations, deletion preconditions, plan assembly, metrics, and reconnection races.
- [ ] Add pure tests for Unicode rendering/search, URL encoding and scheme validation, secret-cache expiry, config sync validation, and secure file permissions.
- [ ] Pin CI and Docker toolchains to Rust 1.89, use `--locked`, and align README requirements with the actual toolchain.
- [ ] Make release builds run format/check/test/clippy/audit preflight and verify the tag matches `Cargo.toml`.
- [ ] Pin GitHub Actions to immutable versions or audited SHAs and add release provenance/signatures/checksums.
- [ ] Decide whether Dockerfile and scripts are supported artifacts. If supported, track and test them, run the image as non-root, and include all required runtime tools.
- [ ] Evaluate replacing deprecated `serde_yaml` and document the migration impact.
- [ ] Bound large-cluster memory/CPU costs by avoiding full-store clones per frame, limiting payload sizes, and reducing broad runner log polling.

## Current Baseline

- Branch: `drift-metrics`
- Pull request: https://github.com/fenio/terrarium/pull/24
- CI blocker fixed locally: Clippy `items_after_test_module` tests were moved to the ends of `src/ui/controller_dashboard.rs` and `src/ui/terraform_detail.rs`.
- CI-equivalent checks pass locally: format, build, 88 tests, and strict Clippy.
