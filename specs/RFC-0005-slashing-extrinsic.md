# RFC-0005 — Slashing Extrinsic ABI

**Status:** Draft — ratification target end Q3 2026
**Owner:** Pallet Lead + Verification Lead
**Consumers:** `pallet-slashing`, `pallet-operator-stake`, `validator-replay`, `validator-watcher`, `governance-tools`

## Goal

Define how slashing evidence is submitted, how severity is calculated per-fault, how the dispute window operates, and how slashed stake is escrowed (not burned) until resolution.

## Design rules (from red-team)

1. **Per-detection, not per-epoch.** 100 cheats detected = 100 slash events. (Rule 3.)
2. **Bounded per-incident.** Max 10% single-incident slash.
3. **Cumulative cap.** Max 50% per month per operator.
4. **Dispute window.** 7 days for operator to dispute; 28 days total resolution.
5. **Escrow, not burn.** Slashed stake held in escrow until T+28d.
6. **Transparency.** Every slash on-chain with reason code; appeal mechanism.
7. **Watcher false-positive penalty.** False slashing claim → watcher bond × 10 + 2nd-offense ban.

## Fault codes

```rust
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum FaultCode {
    WrongModel,             // 10% — model_weight_hash mismatch on replay
    WrongResponse,          // 5%  — response_hash mismatch on replay
    LogProbDrift,           // 2%  — log_probs diverge beyond per-tier ε
    CacheReplay,            // 5%  — cache_hit without fresh-compute signal
    QuantizationSwap,       // 10% — actual quant ≠ declared
    KernelPackMismatch,     // 0.5% — non-det kernel in det tier
    DeviceCertCollision,    // 100% — same GPU UUID across coldkeys (rule 7) — special, no dispute
    HeartbeatMiss,          // soft — no slash, just emission decay
    AttestationStale,       // 2%  — operating past CRL grace period
    SanctionsHit,           // 100% — frozen, not burned (rule 16)
    ValidatorCollusion,     // 10% — cross-validator outlier confirmed
    FakeBurn,               // 50% — gateway-side fraud
    BatchOvercommit,        // 10% — gateway claimed mint > burn supports
}

impl FaultCode {
    pub fn base_severity_bps(&self) -> u16 {
        match self {
            WrongModel | QuantizationSwap | ValidatorCollusion | BatchOvercommit => 1000,
            WrongResponse | CacheReplay => 500,
            LogProbDrift | AttestationStale => 200,
            KernelPackMismatch => 50,
            DeviceCertCollision | SanctionsHit => 10000,  // 100%
            FakeBurn => 5000,
            HeartbeatMiss => 0,  // soft
        }
    }
}
```

## Submission extrinsic

```rust
fn submit_slashing_evidence(
    origin: OriginFor<T>,                       // validator or watcher (pool-disjoint)
    operator_id: AccountId,
    fault_code: FaultCode,
    evidence_hash: H256,                        // points to off-chain evidence blob
    related_job_id: Option<H256>,               // for receipt-based faults
    related_receipt_hash: Option<H256>,
    validator_signatures: BoundedVec<(AccountId, Signature), ConstU32<8>>,
                                                // co-signatures for high-severity claims
) -> DispatchResult;
```

## Multi-signature requirement by severity

| Severity | Co-signature count required |
|---|---|
| 0.5% (KernelPackMismatch) | 1 (submitter only) |
| 2–5% | 2 (submitter + 1 corroborator) |
| 10% | 3 corroborators |
| 50% (FakeBurn) | 3 corroborators + gateway-burn-engine signed evidence |
| 100% (DeviceCertCollision, SanctionsHit) | 5 corroborators OR multisig override |

Corroborators must be:
- For validator-class faults (WrongModel, LogProbDrift, etc.): top-K validator set, geographically distinct.
- For gateway-class faults (FakeBurn, BatchOvercommit): different gateways or independent burn-engine instances.
- For sanctions: foundation multisig fast-track (3-of-7).

## Slash flow

1. Evidence submitted → extrinsic validated → `Slashing` event emitted.
2. Operator's stake equal to `severity_bps × stake / 10000` moved to **escrow account**, not burned.
3. Operator notified via off-chain event subscription.
4. Operator has 7 days to file `dispute_slashing(slash_id, dispute_bond_amount, counter_evidence_hash)` (RFC-0005 §dispute below).
5. If no dispute, at T+7d slash moves from escrow to **slashing-result account** (still not burned, waits 21 more days).
6. At T+28d, slash burns from slashing-result account, unless dispute resolved in operator's favor.

## Dispute protocol

Implementation summary:

```rust
fn dispute_slashing(
    origin: OriginFor<T>,                       // must be the slashed operator
    slash_id: u64,
    dispute_bond: BalanceOf<T>,                 // 10% of slash amount
    counter_evidence_hash: H256,
) -> DispatchResult;

fn arbitrate_dispute(
    origin: OriginFor<T>,                       // must be sortition-selected panelist
    slash_id: u64,
    vote: ArbitrationVote,                      // Uphold | Overturn | Insufficient
    rationale_hash: H256,
) -> DispatchResult;

fn ratify_dispute(
    origin: OriginFor<T>,                       // foundation multisig
    slash_id: u64,
    multisig_signatures: BoundedVec<Signature, ConstU32<7>>,
    decision: MultisigDecision,
) -> DispatchResult;
```

Panel selection: on-chain sortition from top-50 stake, excluding (slashed operator, slashing validator, anyone in same operator's coldkey group). Each panelist posts 1% stake bond.

## Caps and rate limits

- **Single-incident cap:** 10% (1000 bps), enforced at extrinsic level.
- **Monthly cumulative cap:** 50% per operator (5000 bps over 30 rolling days), enforced by checking `MonthlySlashAccumulator` storage.
- **Daily cap:** 30% (3000 bps over 24 hours), prevents runaway cascades.
- **Circuit breaker:** if network-wide slashing exceeds 3× 24-hour rolling baseline, `pallet-slashing` enters paused state requiring 5-of-7 multisig + 2-day public delay to resume (IR playbook §8.2 #2).

## Watcher bond and false-claim penalty

```rust
fn register_watcher(origin: OriginFor<T>, bond_amount: BalanceOf<T>) -> DispatchResult;
                                                // ≥ 1 ETH-equivalent in OROG
```

If watcher's evidence is rejected by panel or auto-validator:

- 1st offense: bond burned (full bond).
- 2nd offense in 90 days: bond × 10 penalty (stake-bonded if registered as both watcher AND operator) + permanent watcher ban.
- Malicious / forged evidence: criminal referral.

## Transparency

Every slash + dispute + ratification is on-chain. `governance-tools` provides a public dispute panel UI showing:

- Slash event + fault code + evidence URL.
- Operator dispute filing.
- Panel composition + votes.
- Multisig ratification.
- Final disposition.

## Versioning

`version` carried in `FaultCode` enum reserved variants. Additive only; no breaking changes without runtime upgrade.

## Open questions

- Should sortition exclude operators registered in last N blocks (anti-flash-panel-stack)? Default: yes, exclude operators registered in last 14 days from panel selection.
- Should panel bond be returned with interest if vote aligns with final disposition? Default: yes, small reward (1% of slash amount split among aligned panelists).
