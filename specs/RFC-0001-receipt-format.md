# RFC-0001 — Signed Response Receipt Format

**Status:** Draft — ratification target end Q3 2026
**Owner:** Verification Lead
**Consumers:** `validator-replay`, `validator-watcher`, `gateway-burn-engine`, `pallet-job-market`, `customer-sdk-{py,ts}`, `attestation-explorer`
**Cross-cuts:** RFC-0002 (attestation report), RFC-0004 (batch settlement), RFC-0005 (slashing extrinsic), RFC-0007 (nonce protocol)

## Goal

Define the canonical signed response receipt emitted by an operator after each inference. Receipts are the unit of work measurement, dispute, and BME settlement.

## Fields

```rust
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct Receipt {
    pub version: u8,                    // 1
    pub job_id: H256,                   // unique per inference
    pub operator_id: AccountId,         // operator hotkey
    pub model_id: H256,                 // base model content hash from pallet-model-registry
    pub model_weight_hash: H256,        // exact weight tensor hash served (catches quant swap)
    pub adapter_id: Option<H256>,       // LoRA adapter, if any
    pub customer_nonce: H256,           // RFC-0007 nonce
    pub request_hash: H256,             // SHA-256 of canonical-serialized request
    pub response_hash: H256,            // SHA-256 of canonical-serialized response
    pub log_probs_sample: Vec<u8>,      // first 64 token log-prob distributions (Targon-style)
    pub kv_metadata: KvMetadata,        // prefix hint, cache state, see below
    pub kernel_pack_hash: H256,         // serving-engine + kernel version pinning
    pub gpu_model: BoundedString<32>,   // "H100-SXM-80GB"
    pub driver_version: BoundedString<32>,
    pub cuda_version: BoundedString<32>,
    pub attestation_report_hash: H256,  // points to pallet-attestation-registry entry (RFC-0002)
    pub batch_invariant_proof: Option<H256>, // SGLang det-mode proof, if applicable
    pub timestamp_ms: u64,
    pub gateway_id: AccountId,          // who routed this job
    pub operator_signature: Signature,  // ed25519 over BLAKE2-256(canonical_encode(everything above))
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct KvMetadata {
    pub prefix_hint: Option<H256>,    // first 1024 token hash for KV-aware routing
    pub cache_hit: bool,              // did we hit KV cache
    pub kv_blocks_used: u32,          // for capacity tracking
}
```

## Encodings

- **On-chain:** SCALE codec. Stored only as Merkle root in batch settlement (RFC-0004). Individual receipts off-chain.
- **Off-chain blob:** JSON with the same field names, base64 for hashes and signatures. Stored in `chain-indexer` archival + IPFS pin for receipt cold-archive.
- **Wire (customer SDK → gateway):** JSON.
- **Wire (operator → gateway):** JSON for HTTP, SCALE for gRPC.

## Signature

```
sig = ed25519_sign(operator_hotkey, BLAKE2-256(SCALE-encode(receipt_without_signature_field)))
```

Verification path: `validator-replay` reads receipt, derives canonical bytes, verifies `operator_signature` against `operator_id` hotkey registered in `pallet-operator-stake`.

## Replay protocol

A validator that samples this receipt (RFC-0006 random selection):

1. Pull receipt blob from `chain-indexer` or operator's gateway-pinned URL.
2. Re-fetch `(model, adapter)` from `pallet-model-registry` content-addressed URLs.
3. Replay using `(request_hash, model_weight_hash, kernel_pack_hash, gpu_model, driver_version, cuda_version)` to match operator's environment within per-tier float ε.
4. Re-compute `response_hash` and `log_probs_sample`.
5. Compare to receipt. Mismatch → slashing extrinsic (RFC-0005).

## Limits

- `log_probs_sample`: capped at 64 tokens × 16-bit indices × top-5 probs = 640 bytes. Sufficient for Targon-style check; bounded for chain cost.
- Total receipt size: ~1.5 KiB worst case.

## Errors / dispute

If operator signature is invalid → receipt is dropped, no slash (not chain-verifiable that operator was even involved). If signature valid but content mismatches replay → slashing extrinsic with severity per-fault-code:

| Fault code | Severity | Example |
|---|---|---|
| `WrongModel` | 10% | model_weight_hash ≠ replayed |
| `WrongResponse` | 5% | response_hash ≠ replayed |
| `LogProbDrift` | 2% | log_probs diverge beyond ε |
| `CacheReplay` | 5% | cache_hit=true but no fresh-compute signal |
| `KernelPackMismatch` | 0.5% | non-deterministic kernel used in deterministic tier |

## Versioning

`version: u8 = 1` at TGE. Increment on any breaking field change; gateway and worker daemon must accept current + previous version for one runtime cycle.

## Open questions

- Should `log_probs_sample` be operator-chosen 64 tokens or randomly-chosen (commit-reveal) for stronger adversary model? Default: random via per-receipt seed = `BLAKE2-256(customer_nonce || job_id)`.
- Should `request_hash` cover raw text or canonicalized chat-template-applied form? Default: canonicalized; chat-template version pinned in `kernel_pack_hash`.

## Backward compatibility

None at TGE. Post-TGE, additive fields only via `version` bump + tail-padding rules.
