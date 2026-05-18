# RFC-0009 — Operator Registration Flow

**Status:** Draft — ratification target end Q3 2026
**Owner:** Pallet Lead + Compliance Lead
**Consumers:** `pallet-operator-stake`, `pallet-attestation-registry`, `sanctions-screener`, `operator-onboarding-ui`, `gateway-router`
**Cross-cuts:** RFC-0002 (attestation matrix per tier), RFC-0003 (heartbeat capabilities), RFC-0005 (collision = slash), RFC-0008 (TWAP for USD-pegged stake)

## Goal

Define how an operator joins the network: which artifacts they must submit, how the chain validates them, what determines their tier, and how upgrades/downgrades flow. Targets red-team rules 7 (device-cert collisions), 16 (sanctions screening at onboarding).

## Identity hierarchy

```
coldkey   — long-term holding; stakes; cannot serve traffic directly
└── hotkey — operational signer for receipts, heartbeats, slashing extrinsics
    └── device_cert — silicon-bound (NVIDIA Device Identity CA), 1:1 with GPU
```

One coldkey may register multiple hotkeys (multi-operator). One hotkey is 1:1 with one device_cert at any time. Re-binding requires deregistration first.

## Tiers (recap from RFC-0002)

| Tier | Attestation requirement | Min stake (USD-pegged) | Notes |
|---|---|---|---|
| `dc-premium` | NVIDIA CC + (TDX OR SEV-SNP) | $5,000 | Highest emission share |
| `dc-standard` | NVIDIA CC + (TDX OR SEV-SNP) | $2,500 | Standard data-center |
| `cloud-rented` | NVIDIA CC + Intel TDX | $2,500 | Cloud GPU; H100/H200 |
| `prosumer` | none (stake-only) | $2,000 | Best-effort tier |
| `edge` | none (stake-only) | $1,500 | Edge / consumer GPUs |
| `embed-only` | none | $1,000 | Whisper / SD / small models |
| `compliance` | NVIDIA CC + TDX + SEV-SNP + SOC 2 | $25,000 | HIPAA/PCI workloads |

Stake denominated per DECISIONS.md H9 default: **OROG with USD-pegged ratchet via 5-of-7 multisig governance every 30 days, using `CurrentTwap` (RFC-0008) as reference price**.

## Registration extrinsic

```rust
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct OperatorRegistration {
    pub version: u8,                            // 1
    pub coldkey: AccountId,
    pub hotkey: AccountId,
    pub attestation_quote: AttestationReportHash, // == OnChainAttestation.report_hash, RFC-0002
    pub tier: OperatorTier,
    pub stake_amount: BalanceOf<T>,             // in OROG native units
    pub geo_region: BoundedString<8>,           // ISO-3166-1 alpha-2 + subdivision
    pub ip_24_hash: H256,                       // BLAKE2(public IP /24) for diversity enforcement
    pub sanctions_check_proof: SanctionsCheckProof,
    pub coldkey_signature: Signature,           // over canonical-encode(everything above)
    pub hotkey_signature: Signature,            // separate, proves hotkey control
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct SanctionsCheckProof {
    pub version: u8,
    pub provider_id: u8,                        // 0=Chainalysis 1=TRM 2=Elliptic
    pub screened_address: AccountId,            // must == coldkey
    pub screened_at_ts: u64,                    // must be ≤ 24h before tx
    pub passed: bool,                           // must be true to register
    pub screener_signature: Signature,          // signed by `sanctions-screener` service key
}

fn register_operator(
    origin: OriginFor<T>,                       // any signer; bound to coldkey via signature
    registration: OperatorRegistration,
) -> DispatchResult;
```

## Validation steps

In `pallet-operator-stake::register_operator`:

1. **Signatures.** Verify `coldkey_signature` against `coldkey`; verify `hotkey_signature` against `hotkey` (separate-key proof-of-control).
2. **Attestation.** Look up `attestation_quote` in `pallet-attestation-registry`. Must be:
   - Not expired (`expires_at > now`).
   - Not revoked.
   - `vendor_set` satisfies the tier requirement (RFC-0002 matrix).
   - `OnChainAttestation.operator_id == hotkey`.
3. **Device-cert collision (rule 7).** Look up `OnChainAttestation.gpu_uuid` across all existing registrations.
   - If found AND under a different coldkey → **both coldkeys slashed 100%** via `pallet-slashing::submit_slashing_evidence(..., FaultCode::DeviceCertCollision, ...)`. No grace, no dispute.
   - If found under same coldkey → reject as duplicate registration.
4. **Stake.** `stake_amount ≥ tier_min_stake_useful()` where the per-tier minimum is computed at extrinsic time from `CurrentTwap`. Stake transferred from `coldkey` to `pallet-operator-stake` reserved balance.
5. **Sanctions.** Verify `sanctions_check_proof`:
   - `screener_signature` signed by an authorized screener key (allowlist in `pallet-treasury-ext`).
   - `screened_at_ts` within 24h.
   - `passed == true`.
   - `screened_address == coldkey`.
   - Reject otherwise. Cached "passed" results are not accepted from a third party.
6. **Geo / IP diversity.** Check `(geo_region, ip_24_hash)` against `OperatorDiversityCaps`:
   - No more than 5% of total active operators in any single `ip_24_hash`.
   - No more than 20% in any single `geo_region`.
   - Soft caps: registration accepted but flagged `diversity_warning=true`; emission share reduced 20% while over cap.
7. **Tier-stake binding.** Insert into `OperatorRecords` map; emit `OperatorRegistered(coldkey, hotkey, tier, attestation_quote)`.

## Storage

```rust
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct OperatorRecord {
    pub coldkey: AccountId,
    pub hotkey: AccountId,
    pub tier: OperatorTier,
    pub stake_useful: BalanceOf<T>,
    pub attestation_report_hash: H256,
    pub geo_region: BoundedString<8>,
    pub ip_24_hash: H256,
    pub registered_at: BlockNumber,
    pub last_heartbeat_at: BlockNumber,
    pub status: OperatorStatus,         // Pending | Active | Suspended | Deregistering | Slashed
    pub diversity_warning: bool,
}

StorageMap:        OperatorRecords     hotkey   -> OperatorRecord
StorageMap:        ColdkeyOperators    coldkey  -> BoundedVec<AccountId, ConstU32<32>>  // hotkeys
StorageMap:        GpuUuidIndex        H256     -> AccountId  // hotkey owning this GPU
StorageMap:        Ip24Counts          H256     -> u32
StorageMap:        GeoRegionCounts     BoundedString<8> -> u32
StorageValue:      DiversityCaps       DiversityCapsConfig
```

## Tier upgrade / downgrade

```rust
fn upgrade_tier(
    origin: OriginFor<T>,                       // coldkey
    hotkey: AccountId,
    new_tier: OperatorTier,
    additional_stake: BalanceOf<T>,             // may be 0 if existing stake covers new min
    new_attestation_quote: Option<H256>,        // required if new tier has stricter attestation
) -> DispatchResult;

fn downgrade_tier(
    origin: OriginFor<T>,
    hotkey: AccountId,
    new_tier: OperatorTier,
) -> DispatchResult;
```

Upgrade is effective immediately if attestation + stake satisfy new tier requirements. Downgrade enters a 7-day cool-down where the operator continues to serve at the lower tier; any pending receipts settle at the previous tier's pricing.

## Deregistration

```rust
fn deregister_operator(
    origin: OriginFor<T>,                       // coldkey
    hotkey: AccountId,
) -> DispatchResult;
```

Flow:
1. Operator status → `Deregistering`.
2. New jobs no longer routed (gateway-side respects the status flag).
3. 14-day unbonding period; stake held in escrow.
4. After 14d, stake returned to coldkey; record archived; `GpuUuidIndex` entry cleared.
5. Any slashing extrinsic accepted during the 14d window resolves first.

## Sanctions re-screening

`sanctions-screener` re-screens every active operator's coldkey:
- Every 24h continuously.
- Within 1h of any inbound transfer >$10K equivalent.
- Real-time on outbound transfers.

A failed re-screen emits `SanctionsHit(coldkey)`; `pallet-slashing` consumes via RFC-0005 `SanctionsHit` fault code (100%, frozen — not burned).

## Hotkey rotation

```rust
fn rotate_hotkey(
    origin: OriginFor<T>,                       // coldkey
    old_hotkey: AccountId,
    new_hotkey: AccountId,
    new_attestation_quote: H256,                // new key bound to same GPU
    coldkey_signature: Signature,
    new_hotkey_signature: Signature,
) -> DispatchResult;
```

Rotation must point to the same `gpu_uuid` to prevent a coldkey from accumulating extra silicon stealthily. `GpuUuidIndex` updated atomically.

## Errors

| Error | When |
|---|---|
| `AttestationInvalid` | quote not in registry, expired, revoked, or wrong vendor_set |
| `DeviceCertCollisionDetected` | gpu_uuid already exists under different coldkey (also triggers slash) |
| `StakeBelowTierMinimum` | stake_amount < tier_min_stake_useful() at TWAP |
| `SanctionsCheckFailed` | screener says fail, or proof invalid/stale |
| `DiversityCapExceeded` | hard cap (not soft warning) hit |
| `TierNotEligibleForRegistration` | tier=embed-only requested but model registry says base model unsupported |
| `ColdkeyHotkeyAlreadyBound` | duplicate registration |

## Versioning

`version: u8 = 1` at TGE. Increment on field changes; one runtime cycle of mixed-version tolerance.

## Open questions

- Whether to allow re-registration after slash + unbond + re-screen, or permanent ban. Default: re-registration allowed after 6 months + new coldkey + fresh sanctions screen — except for `SanctionsHit` (permanent) and `DeviceCertCollision` (permanent for affected coldkey).
- Whether geo-region diversity caps should be governance-mutable. Default: yes, behind 5-of-7 multisig + 14-d timelock.
