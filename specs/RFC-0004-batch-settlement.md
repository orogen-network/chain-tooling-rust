# RFC-0004 — Batch Settlement Format

**Status:** Draft — ratification target end Q3 2026
**Owner:** Pallet Lead
**Consumers:** `pallet-job-market`, `pallet-bme`, `gateway-burn-engine`, `validator-replay`

## Goal

Aggregate ~thousands of per-inference receipts (RFC-0001) into one on-chain settlement extrinsic per epoch per gateway. Reduces 100M tx/day → ~280K tx/day total across the network.

## Settlement cadence

- One batch per epoch (~72 min) per gateway. Multi-gateway operators are encouraged for fault tolerance.
- Receipts older than 2 epochs at submission time are rejected (anti-stale).
- Receipts younger than 1 epoch are accepted (allows gateways to settle eagerly when batch is full).

## Batch structure

```rust
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct SettlementBatch {
    pub version: u8,                              // 1
    pub batch_id: H256,                           // BLAKE2 of all leaves + epoch + gateway
    pub epoch_number: u64,
    pub gateway_id: AccountId,
    pub receipt_count: u32,
    pub merkle_root: H256,                        // root over leaf = BLAKE2(canonical_encode(receipt))
    pub aggregate_burn_cuc: u128,                 // total CUC consumed in this batch
    pub aggregate_mint_useful: u128,              // expected operator mint
    pub per_operator_summary: BoundedVec<OperatorSummary, ConstU32<512>>,
    pub gateway_signature: Signature,             // ed25519
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct OperatorSummary {
    pub operator_id: AccountId,
    pub receipts_count: u32,
    pub aggregate_tokens_served: u64,
    pub aggregate_mint_useful: u128,
    pub merkle_subroot: H256,                     // subroot over just this operator's receipts
}
```

## Extrinsic

```rust
fn submit_batch(
    origin: OriginFor<T>,                         // must be a registered gateway
    batch: SettlementBatch,
    receipts_blob_cdn_url: BoundedString<256>,    // where full receipts are pinned
) -> DispatchResult;
```

Effects:

1. Verify `gateway_signature` against registered gateway hotkey.
2. Verify per-operator aggregates ≤ on-chain stake-pool capacity (anti-overcommit).
3. Verify aggregate mint ≤ epoch mint headroom from `pallet-bme`.
4. Verify aggregate burn ≥ aggregate mint × USD-equivalent ratio (per BME math).
5. Burn CUC from gateway's escrow.
6. Mint OROG to operators per `OperatorSummary`.
7. Emit `SettlementBatchSubmitted` event with `(batch_id, gateway_id, merkle_root, operator_summaries_hash)`.
8. Store batch header (not full content) in `BatchHeaders` storage.

## Dispute window

After submission:

- T+0 to T+24h: any operator can submit `dispute_batch(batch_id, merkle_proof, expected_summary)` if their `OperatorSummary` is missing or wrong. Disputed batches partially refunded; mint reversed via `pallet-bme::reverse_mint`.
- T+0 to T+epoch_end: validators replay sampled receipts (RFC-0006) and submit slashing extrinsics (RFC-0005) for mismatches.

## Receipt blob hosting

`receipts_blob_cdn_url` points to:
- `chain-indexer` archival (operated by foundation + partners).
- IPFS pin via `weight-cdn-pinner` (content-addressed by `batch_id`).
- Gateway's own S3/R2 (mandatory until indexer is decentralized).

Operator and validator must be able to fetch any receipt via Merkle proof against `merkle_root`.

## Sanity checks (chain-side)

- `receipt_count == per_operator_summary.map(|s| s.receipts_count).sum()`.
- `aggregate_burn_cuc == receipts.map(|r| burn_for(r)).sum()` — verified by gateway-burn-engine before signing.
- `aggregate_mint_useful ≤ aggregate_burn_cuc × oracle_rate × subsidy_factor` from `pallet-bme` epoch state.

## Adversary scenarios

1. **Inflated mint claim:** gateway claims more mint than burn supports. Caught at extrinsic validation (step 4). Slash gateway 10% of stake; void batch.
2. **Fake receipts:** gateway includes receipts that operators never signed. Operators dispute via T+24h window; gateway slashed.
3. **Missing receipts:** gateway omits operators' work to save burn. Operators dispute via T+24h window; mint added retroactively + gateway slashed.
4. **Batch collision:** two gateways claim same job_id. Earliest valid wins; later slashed.

## Storage cost

Header per batch: ~10 KiB SCALE. At 280K batches/day = 2.8 GB/day on archive nodes. Pruning policy: keep all headers; full receipts off-chain via CDN.

## Versioning

`version: u8 = 1` at TGE. Increment on field changes. Compat-tested in CI.

## Open questions

- Should batch size be capped (e.g. 10K receipts max)? Default: yes, 10K cap per batch; gateway emits multiple batches if needed.
- Should we support cross-gateway batch aggregation (one batch from multiple gateways)? Default: no at TGE; complicates dispute attribution.
