# RFC-0003 — Heartbeat Schema

**Status:** Draft — ratification target end Q3 2026
**Owner:** Serving Lead
**Consumers:** `gateway-router` (for routing), `validator-watcher` (anomaly detection), `pallet-operator-stake` (liveness)

## Goal

Define the recurring liveness + capability advertisement an operator emits while online. Heartbeats are *not* receipts; they're the lightweight feed that lets the gateway pick capable operators and the chain detect downtime.

## Cadence

- **Off-chain heartbeat** (gateway-bound): every 12 seconds, block-aligned. Pushed over WebSocket from `worker-control-plane` to gateway router fleet.
- **On-chain heartbeat** (liveness anchor): once per epoch (every 360 blocks ≈ 72 minutes). Extrinsic to `pallet-operator-stake::heartbeat()`. Inclusion proves the operator is alive in that epoch; absence triggers liveness penalty (not slashing, just emission share reduction).

## Off-chain heartbeat structure

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct OffChainHeartbeat {
    pub version: u8,                            // 1
    pub operator_id: AccountId,                 // hotkey
    pub block_number: u64,                      // current chain tip the operator saw
    pub capabilities: Vec<Capability>,
    pub current_load: LoadSnapshot,
    pub kv_cache_pressure: f32,                 // 0.0–1.0
    pub last_completed_job_id: Option<H256>,
    pub attestation_freshness: AttestationFreshness,
    pub watchdog_state: WatchdogState,
    pub price_per_million_tokens: u64,          // in CUC micro-units
    pub geo_region: BoundedString<8>,           // ISO-3166-1 alpha-2 + subdiv
    pub signature: Signature,                   // ed25519 over canonical-encode
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Capability {
    pub base_model_id: H256,
    pub adapter_ids: Vec<H256>,                 // currently warm adapters
    pub quantization: Quantization,             // FP16 | FP8 | INT8 | INT4
    pub max_context_tokens: u32,
    pub max_concurrent_requests: u32,
    pub deterministic_mode: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LoadSnapshot {
    pub active_requests: u32,
    pub queue_depth: u32,
    pub p50_ttft_ms: u32,
    pub p99_ttft_ms: u32,
    pub p50_itl_ms: u32,
    pub p99_itl_ms: u32,
    pub gpu_memory_used_gb: f32,
    pub gpu_utilization_pct: f32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AttestationFreshness {
    pub last_attested_at_ms: u64,
    pub expires_at_ms: u64,
    pub current_report_hash: H256,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WatchdogState {
    pub vllm_pid_alive: bool,
    pub vllm_last_log_ms: u64,
    pub last_restart_count_24h: u32,
}
```

## On-chain heartbeat extrinsic

```rust
fn heartbeat(
    origin: OriginFor<T>,
    epoch_number: u64,
    capabilities_summary_hash: H256,    // BLAKE2 of canonical capabilities list
    load_summary: LoadSummary,
    attestation_report_hash: H256,      // current attestation from pallet-attestation-registry
) -> DispatchResult;
```

`LoadSummary` is bounded; `capabilities_summary_hash` is enough to detect drift; full capabilities advertised off-chain (more frequent + bigger).

## Routing semantics

Gateway router maintains an in-memory operator catalog refreshed by off-chain heartbeats. Routing decisions per request:

1. Resolve `(base_model_id, adapter_id?, tier_preference, region_preference, latency_budget, max_price)`.
2. Filter `Capability` set: operators that advertise the requested model + adapter + tier within budget.
3. Score by latency estimate, KV-warm hint, reputation (from yuma scoring), price.
4. Session pinning: if `session_id` is provided and the prior operator is still capable, prefer them.
5. Send job; on operator timeout (>2× p99 TTFT), retry on next-best.

## Validator watcher signals

`validator-watcher` ingests heartbeats and flags:
- Sudden capability churn (operator drops 10+ adapters in 1 hour without re-attestation).
- Load anomaly (p99 TTFT spikes 5× while utilization stays flat — synthetic-batch attack signal).
- Geo-region change without re-attestation (sybil signal).
- Attestation freshness vs. CRL state (operator running stale firmware).

These are *signals*, not slashes; signals feed validator scoring and IR playbook §8.2 #11.

## Bandwidth

12s heartbeat × ~10 KiB payload × ~1000 operators × 100 gateway replicas worst-case = ~84 GiB/day total catalog traffic. Acceptable for gateway-side ingest; aggregated via per-region pub/sub.

## Versioning

`version: u8 = 1` at TGE. Increment on schema change. Mixed-version operators tolerated for one runtime cycle.

## Open questions

- Should off-chain heartbeats be signed by hotkey or by a separate session key for performance? Default: hotkey (operational simplicity), revisit at >5k operators.
- Should we gossip heartbeats over libp2p to other operators (for peer discovery) or keep gateway-only? Default: gateway-only at TGE; libp2p in Phase 4.
