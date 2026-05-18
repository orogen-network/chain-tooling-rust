# RFC-0008 — Price Oracle Feed for BME Settlement

**Status:** Draft — ratification target end Q3 2026
**Owner:** Tokenomics Lead + Pallet Lead
**Consumers:** `pallet-oracle-twap`, `pallet-bme`, `gateway-burn-engine`, `validator-watcher`
**Cross-cuts:** RFC-0004 (batch settlement reads CurrentTwap), RFC-0005 (oracle defection is slashable)

## Goal

Provide a manipulation-resistant USD price feed for OROG and CUC, used by `pallet-bme` to compute the burn:mint ratio at settlement. Targets red-team rule 10: long-window TWAP at launch to prevent flash-loan / first-listing manipulation.

## Sources (4)

1. **Binance** — top-CEX price for the OROG/USDT pair (post-listing).
2. **Coinbase** — second top-CEX for OROG/USDC.
3. **On-chain DEX (Uniswap V3 on Ethereum)** — OROG/USDC pool, observed via Snowbridge → `pallet-oracle-twap` adapter (post-listing).
4. **Uniswap V4 protocol-managed AMM** — bonded foundation-controlled hook-AMM on our own chain (launched at TGE, holds protocol liquidity).

Source set is governance-mutable behind 5-of-7 multisig + 14-d timelock + 30-d cool-down.

## Aggregation algorithm

Per epoch (or finer):

```
1. Collect all source prices submitted in the lookback window.
2. Compute TWAP per source: weighted by time-segment length.
3. Compute the median across the 4 source TWAPs.
4. Compute deviation of each source TWAP from the median.
5. Exclude any source where |dev| > 5% of median.
6. If remaining sources < 2 → fallback (see below).
7. Aggregate TWAP = stake-weighted mean of remaining sources.
       (oracle pool stake-weight; foundation pool default-equal)
8. Write CurrentTwap.
```

## Lookback window

Per red-team rule 10:

| Phase | Window | Notes |
|---|---|---|
| Months 0–18 post-TGE | 4–12 hours | Tunable by oracle ops within bounds; default 8h. Long window blunts flash-loan and thin-market attacks at fresh listings. |
| Months 19+ | 30 minutes | Step-down via governance proposal; cannot be set <30 min without a runtime-upgrade RFC. |

Window changes require 5-of-7 multisig + 14-d timelock; no emergency reduction below 30 min.

## On-chain types

```rust
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct PriceSample {
    pub version: u8,                  // 1
    pub source_id: u8,                // 0=Binance 1=Coinbase 2=UniV3-ETH 3=UniV4-local
    pub price_usd_q64: u128,          // OROG→USD as fixed-point Q64.64
    pub volume_usd_q64: u128,         // 0 for AMM TWAP; nonzero for CEX samples
    pub ts_ms: u64,
    pub oracle_id: AccountId,         // submitter
    pub signature: Signature,
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct AggregatedTwap {
    pub version: u8,
    pub computed_at_block: BlockNumber,
    pub window_start_ms: u64,
    pub window_end_ms: u64,
    pub price_usd_q64: u128,
    pub contributing_sources: BitFlags8,
    pub excluded_sources: BitFlags8,    // those clipped as outliers
    pub fallback: bool,                 // true if static-fallback active
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct OraclePoolMember {
    pub oracle_id: AccountId,
    pub stake: BalanceOf<T>,           // separate stake pool; min 10× operator min stake
    pub registered_at: BlockNumber,
    pub good_submissions: u64,
    pub bad_submissions: u64,
    pub last_submitted_at: BlockNumber,
}
```

## Storage

```rust
StorageMap:   OraclePool       AccountId -> OraclePoolMember
StorageMap:   PriceHistory     (source_id, ts_bucket) -> BoundedVec<PriceSample, ConstU32<32>>
StorageValue: CurrentTwap      AggregatedTwap
StorageValue: WindowMs         u64              // current TWAP window
StorageValue: EmergencyPaused  bool             // multisig kill-switch
StorageValue: LastGoodPrice    AggregatedTwap   // for static fallback
```

`PriceHistory` is pruned beyond `WindowMs × 2` lookback.

## Extrinsics

```rust
/// Oracle pool member submits a per-source price sample.
fn submit_price(
    origin: OriginFor<T>,            // must be in OraclePool
    sample: PriceSample,
) -> DispatchResult;

/// Recompute the aggregated TWAP. Permissionless callable but rate-limited;
/// also auto-called on every batch settlement via on_initialize.
fn refresh_twap(origin: OriginFor<T>) -> DispatchResult;

/// 5-of-7 multisig: emergency pause minting. Freezes pallet-bme mint path
/// but does NOT halt burns or operator emissions from prior epochs.
fn emergency_pause(origin: OriginFor<T>) -> DispatchResult;

fn emergency_unpause(
    origin: OriginFor<T>,
    multisig_signatures: BoundedVec<Signature, ConstU32<7>>,
) -> DispatchResult;

/// Governance: rotate oracle pool membership.
fn add_oracle(origin: OriginFor<T>, oracle_id: AccountId, stake: BalanceOf<T>) -> DispatchResult;
fn remove_oracle(origin: OriginFor<T>, oracle_id: AccountId, reason_hash: H256) -> DispatchResult;
```

## Oracle pool

- ≥5 active oracle members at all times. Below 5 → `emergency_pause` auto-triggered by `on_initialize`.
- Stake: ≥10× operator minimum (denomination follows DECISIONS.md H9 default — OROG with USD-pegged ratchet every 30 days).
- Rotation: term-limited 90 days; minimum 30 days between re-add by same operator.
- Geo-distributed: ≥3 distinct jurisdictions among the active set.
- Disjoint from validators and gateways at TGE.

## Fallback logic

Trigger | Action
---|---
≥2 outlier sources clipped in one TWAP cycle | Use `LastGoodPrice` (≤ 24h old); emit `OracleFallbackActive`
Source unreachable for >30 min | Mark source `inactive`; auto-redrop after 1h healthy heartbeat
3 of 4 sources unreachable | `emergency_pause()` auto-triggered; multisig must `emergency_unpause`
`LastGoodPrice` older than 24h AND no live aggregate | Halt mint path entirely; burns still allowed; IR runbook 03

`pallet-bme::emergency_pause()` is the user-visible kill-switch; gated 5-of-7 multisig.

## Defection penalties

| Behavior | Penalty |
|---|---|
| Submitted price ≥10% from final agg (per-submission outlier) | Increment `bad_submissions`; reputation decay |
| `bad_submissions / total ≥ 20%` over rolling 30 days | Auto-eject from pool; 25% stake slash (`FakeBurn` analog routed through RFC-0005 with custom code `OracleManipulation`) |
| Failed to submit for >12 hours while in pool | Soft penalty: half-emission for that epoch |
| Provable collusion (price agreement off-chain leaked) | 100% slash; permanent ban |

Slashing for oracle pool flows through RFC-0005 with the new fault-code variant `OracleManipulation` (severity 2500 bps base, scalable).

## Manipulation cost analysis

To shift the aggregated TWAP by >5% with the 8h window:

- Attacker must hold majority position in ≥3 of 4 sources for ≥4h continuous.
- For UniV4 protocol-managed AMM: liquidity depth is foundation-controlled at TGE (≥$5M) — attacker needs to absorb that depth.
- For CEX sources: 4h volume-weighted shift requires multi-million-dollar sustained pressure; CEX surveillance flags it.
- Cost asymmetry: oracle stake slash + multi-source manipulation cost >> potential mint inflation gain because BME caps headroom independently.

## Bandwidth

- 4 sources × 1 sample/source/min × 4 oracle pool members = 16 extrinsics/min nominally.
- ~96 bytes/sample × 16 × 60 × 24 = 2.1 MiB/day on-chain.
- Pruning maintains `PriceHistory` at ~1 day worst case.

## Versioning

`version: u8 = 1` at TGE. Source matrix governance-mutable; window governance-mutable bounded. Breaking changes require RFC bump.

## Open questions

- Whether to add Chainlink as a 5th source. Default: no at TGE; consider Q3 2027 once their cross-chain pricing rails are mature on Snowbridge.
- Whether to publish source-level TWAPs publicly. Default: yes — `status-page` shows source TWAPs + agg + clipped flags real-time.
