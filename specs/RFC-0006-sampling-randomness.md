# RFC-0006 — Commit-Reveal Sampling Randomness

**Status:** Draft — ratification target end Q3 2026
**Owner:** Verification Lead
**Consumers:** `validator-replay`, `validator-watcher`, `pallet-yuma-consensus`, `pallet-slashing`, `chain-indexer`
**Cross-cuts:** RFC-0001 (receipt → leaf for selection), RFC-0004 (batch → input to seed), RFC-0005 (slashing for non-reveal)

## Goal

Define the unpredictable, unmanipulable per-epoch randomness that drives validator replay sampling. The seed must be:

- **Bias-resistant** — no single validator can steer it.
- **Predictable in cadence** — gateways and operators know when a sample is finalized.
- **Cheap to verify** — every full node recomputes it in O(N_validators) hash ops.
- **Late-binding** — receipts cannot be selectively retained/discarded by the operator after the seed is known.

## Design rules (from red-team)

1. **Sample rate ≥10% floor** (rule 2). Per-tier ceilings: `edge` 25%, `prosumer` 20%, `cloud-rented` 15%, `dc-standard` 12%, `dc-premium` 10%, `compliance` 10%.
2. **One-epoch commit-reveal delay** (rule 2). Validator commits at epoch E, reveals at E+1; sampling resolves over receipts in epoch E (closed by then).
3. **Stake-weighted Fisher-Yates** over the operator set; not uniform — high-stake operators bear more verifier load proportionally.
4. **Defection penalties scale.** Single miss → 1 epoch emissions; second within rolling 30 epochs → permit revoked via `pallet-yuma-consensus::revoke_validator`.

## On-chain types

```rust
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct EpochCommitment {
    pub version: u8,                  // 1
    pub epoch_number: u64,            // the epoch the commitment is FOR (i.e., E)
    pub validator_id: AccountId,      // hotkey
    pub commitment_hash: H256,        // BLAKE2-256(preimage || validator_id || epoch_number)
    pub submitted_at: BlockNumber,
    pub signature: Signature,         // ed25519 over canonical-encode
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct EpochReveal {
    pub version: u8,                  // 1
    pub epoch_number: u64,            // same E the commitment was for
    pub validator_id: AccountId,
    pub preimage: [u8; 32],           // 256-bit random
    pub submitted_at: BlockNumber,    // must be in epoch E+1
    pub signature: Signature,
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct SampleAssignment {
    pub version: u8,                  // 1
    pub epoch_number: u64,            // the epoch whose receipts are being sampled (= E)
    pub validator_id: AccountId,      // who replays
    pub operator_id: AccountId,       // whose work is replayed
    pub batch_id: H256,               // RFC-0004 batch
    pub receipt_indices: BoundedVec<u32, ConstU32<1024>>,  // indices into batch merkle leaves
    pub deadline_block: BlockNumber,  // must submit slashing or attest-clean by then
}
```

## Extrinsics

```rust
/// Validator commits to a preimage for epoch E. Must arrive before the first block of E+1.
fn commit_epoch_seed(
    origin: OriginFor<T>,                // validator hotkey
    epoch_number: u64,
    commitment_hash: H256,
) -> DispatchResult;

/// Validator reveals preimage for epoch E. Must arrive in epoch E+1.
fn reveal_epoch_seed(
    origin: OriginFor<T>,
    epoch_number: u64,
    preimage: [u8; 32],
) -> DispatchResult;

/// Any full node can submit a sample assignment derived from the now-known epoch random
/// for any batch in epoch E. Idempotent; first valid submission persisted.
fn finalize_sample(
    origin: OriginFor<T>,
    epoch_number: u64,
    batch_id: H256,
    assignments: BoundedVec<SampleAssignment, ConstU32<256>>,
) -> DispatchResult;
```

`commit_epoch_seed` rejected if (validator not in top-K=128) or (already committed for this epoch) or (epoch already finalized).
`reveal_epoch_seed` rejected if (no commitment) or (preimage hash ≠ commitment) or (out of window).
`finalize_sample` rejected if assignments do not match the deterministic algorithm below.

## Seed derivation

After the reveal window closes at end of epoch E+1:

```
epoch_random[E] = BLAKE2-256( concat_sorted_by_validator_id(reveals) )
```

Validators who failed to reveal are excluded from the concatenation; their commitments are recorded for slashing accounting but contribute nothing to the random.

If `epoch_random` would have <3 contributing reveals → fall back to `epoch_random[E] = BLAKE2-256(epoch_random[E-1] || E)` AND emit a `RandomnessDegraded` event. Two consecutive degraded epochs trigger `pallet-system::set_safe_mode(true)` per IR runbook 02.

## Per-receipt sample selector

For each `(epoch_number, batch_id, operator_id)`:

```
seed = BLAKE2-256(
    epoch_random[E]      ||
    receipt_merkle_root  ||      // from SettlementBatch (RFC-0004)
    batch_id             ||
    operator_id
)
```

Stake-weighted Fisher-Yates draws from the operator's stake-weighted receipts in the batch:

```
weights[i] = operator_stake[operator_id_of(receipt_i)] / total_operator_stake_in_batch
target_count = ceil(operator_receipts_in_batch * tier_sample_rate)
```

`tier_sample_rate ∈ {0.25, 0.20, 0.15, 0.12, 0.10, 0.10}` keyed off `OperatorTier` (RFC-0002).

Validator-to-sample assignment uses a second draw with stake weighting over the active validator set; each validator gets a quota of receipts to replay roughly proportional to its validator stake, with a floor of 1 receipt per active validator per epoch.

The selection algorithm is normative — any two full nodes must produce the same `SampleAssignment` set given the same chain state. Reference implementation lives in `validator-replay::sampling::fisher_yates_stake_weighted`.

## Defection penalties

| Event | Penalty |
|---|---|
| Failed to commit (epoch E) | 1 epoch of validator emissions forfeit; recorded in `MissedCommits[E][validator]` |
| Failed to reveal (epoch E+1) after committing | 1 epoch of emissions forfeit + reputation decay (−0.05 in Yuma score) |
| 2nd defection within 30 rolling epochs | Validator permit revoked via `pallet-yuma-consensus::revoke_validator`; stake unbonds with the standard 14-day delay |
| Grinding attempt (multiple commits per epoch detected at gossip layer) | 10% slash via RFC-0005 `ValidatorCollusion` fault code |

Defection events emit `ValidatorDefected(epoch, validator_id, reason)` consumed by `validator-watcher` for scoring.

## Storage

```rust
StorageMap: EpochCommitments  (epoch, validator) -> EpochCommitment
StorageMap: EpochReveals      (epoch, validator) -> EpochReveal
StorageMap: EpochRandom       epoch              -> H256        // computed at end of E+1
StorageMap: MissedCommits     epoch              -> BoundedVec<AccountId, ConstU32<128>>
StorageMap: MissedReveals     epoch              -> BoundedVec<AccountId, ConstU32<128>>
StorageDoubleMap: SampleAssignments (epoch, batch_id) -> BoundedVec<SampleAssignment, ConstU32<256>>
StorageMap: DegradedEpochs    epoch              -> bool
```

Commitments + reveals pruned after T+epoch_length × 4 to bound state growth. `EpochRandom` retained for 90 days (audit + dispute reconstruction).

## Adversary scenarios

1. **Last-revealer grinding.** Last validator to reveal could choose between revealing or aborting to bias outcome. Mitigation: penalty for non-reveal scales (loss of 1 epoch emissions ≈ ≥10× expected grinding benefit at any realistic stake). Top-K=128 means a single defector shifts ~1/128 of the entropy mass.
2. **Coalition non-reveal.** A coalition of M validators refuses to reveal hoping to force the degraded fallback (which depends on previous epoch). Mitigation: degraded fallback exists but second consecutive degraded epoch trips safe-mode; coalition cost is total emission forfeit.
3. **Commitment grinding via fake identities.** Solved by stake gating on validator set (top-K) and stake-weighted entropy contribution.
4. **Operator pre-mining receipts to avoid sample.** Receipts in batch (RFC-0004) are committed before E ends and merkle-rooted before E+1's reveals are known. Operator cannot retroactively remove a receipt.
5. **Validator-operator collusion.** Validator skews receipt selection toward operator's "safe" receipts. Mitigation: algorithm is deterministic; deviation is provable on-chain and slashable.

## Performance budget

- Commit extrinsic: ~50 bytes; ~50 μs CPU.
- Reveal extrinsic: ~50 bytes; ~50 μs CPU; verifies preimage against stored commitment.
- Finalize-sample: O(N_receipts_in_batch) Fisher-Yates, ~10K receipts ≈ 5 ms. Bounded by `BoundedVec<_, 256>` assignment cap per call.
- Worst case 128 validators × 2 extrinsics × 360 blocks-per-epoch = 256 randomness extrinsics every 72 minutes, well within block weight.

## Versioning

`version: u8 = 1` at TGE. Increment on any field change. Old commitments still resolvable by old algorithm during one runtime cycle of overlap.

## Open questions

- Whether to use VRFs instead of commit-reveal once a stable VRF beacon (drand mainnet bridge or Sassafras) is ratified. Default: commit-reveal at TGE; migrate to VRF in Phase 4.
- Whether `tier_sample_rate` ceilings should be governance-mutable. Default: yes, behind 5-of-7 multisig + 14-d timelock.
