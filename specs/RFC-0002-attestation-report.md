# RFC-0002 — Combined Multi-Vendor Attestation Report

**Status:** Draft — ratification target end Q3 2026
**Owner:** Security Lead
**Consumers:** `attestation-service`, `pallet-attestation-registry`, `validator-replay`, `operator-onboarding-ui`

## Goal

Define how Intel TDX + AMD SEV-SNP + NVIDIA H100/H200/B200 CC attestation quotes are combined into one signed report, hashed, and stored on-chain.

## Why multi-vendor

Red-team rule 8: single-vendor PKI compromise should not collapse the network. A combined report binds the operator to *all* attesting vendors simultaneously; a verifier rejects if any required vendor's quote is missing or invalid.

## Vendor matrix per tier

| Tier | Required quotes | Optional quotes |
|---|---|---|
| `dc-premium` | NVIDIA CC + (Intel TDX OR AMD SEV-SNP) | second CPU vendor |
| `dc-standard` | NVIDIA CC + (Intel TDX OR AMD SEV-SNP) | — |
| `cloud-rented` | NVIDIA CC + Intel TDX | — |
| `prosumer` / `edge` / `embed-only` | none (stake-only sybil resistance) | — |
| `compliance` (HIPAA/PCI tier) | NVIDIA CC + Intel TDX + AMD SEV-SNP | SOC 2 cert hash |

## Report structure

```rust
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct AttestationReport {
    pub version: u8,                  // 1
    pub operator_id: AccountId,
    pub tier: OperatorTier,
    pub gpu_quote: Option<NvidiaQuote>,
    pub tdx_quote: Option<IntelTdxQuote>,
    pub sev_snp_report: Option<AmdSevSnpReport>,
    pub rim_attestation: Option<NvtrustRimAttestation>,
    pub firmware_hashes: BoundedVec<H256, ConstU32<16>>,  // for CRL check
    pub measured_vm_bundle: H256,     // what's running inside the enclave
    pub timestamp_ms: u64,
    pub validity_window_ms: u64,      // re-attest required after this elapses
    pub aggregator_signature: Signature, // signed by attestation-service
    pub vendor_pki_chain_hashes: BoundedVec<H256, ConstU32<8>>, // for CRL lookup
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct NvidiaQuote {
    pub device_cert: BoundedVec<u8, ConstU32<8192>>,
    pub attestation_cert: BoundedVec<u8, ConstU32<8192>>,
    pub measurement: H256,
    pub nonce: H256,                 // anti-replay
    pub gpu_uuid: H256,              // silicon-bound identity
}

// IntelTdxQuote and AmdSevSnpReport follow vendor formats; canonicalized hashes stored on-chain
```

## Storage

`pallet-attestation-registry` stores only:

```rust
pub struct OnChainAttestation {
    pub operator_id: AccountId,
    pub report_hash: H256,           // = BLAKE2-256(SCALE-encode(report_without_signature))
    pub gpu_uuid: H256,              // for L1 device-cert collision detection (red-team rule 7)
    pub vendor_set: BitFlags8,       // which vendors are in this report
    pub measured_vm_bundle: H256,
    pub expires_at: BlockNumber,
    pub revoked: bool,
}
```

Full report blob lives in `chain-indexer` + IPFS. `attestation-service` produces report, computes hash, calls `pallet-attestation-registry::submit(report_hash, gpu_uuid, vendor_set, measured_vm_bundle, expires_at)` via aggregator-signed extrinsic.

## CRL (Certificate Revocation List)

```rust
pub struct CrlEntry {
    pub kind: CrlKind,    // FirmwareHash | DeviceCert | ModelHash | VendorPkiChain
    pub target: H256,
    pub reason: BoundedVec<u8, ConstU32<256>>,
    pub added_at: BlockNumber,
    pub grace_until: BlockNumber,
}
```

CRL writes are multisig-gated (5-of-7) with the standard 14-day timelock — *except* for sanctions hits (3-of-7 fast-track) and known-CVE firmware hashes (3-of-7 fast-track per IR playbook §8.2 #5).

Operators check CRL via `pallet-attestation-registry::is_revoked(report_hash | gpu_uuid | firmware_hash)` every 10 minutes and on every job start.

## Re-attestation

- Every 7 days minimum (validity_window default = 7 × 86400 × 1000 ms).
- Immediately after any CRL update affecting this operator.
- Triggers a re-submit; old report_hash is `revoked=true` but receipts older than the new attestation are honored.

## Vendor PKI chain validation (off-chain)

`attestation-service` validates against:

- **NVIDIA NVTrust** — RIM service, Device Identity CA, Attestation CA. Chain pulls latest CA bundle from a foundation-hosted pinned mirror; updates via governance.
- **Intel Trust Authority** — quote signing CA, fmspc-bound endorsements.
- **AMD KDS** — root-of-trust certificate, ASK certificate.

A vendor's PKI chain compromise → governance push to CRL all `vendor_pki_chain_hashes` matching the compromised root.

## Side-channel disclosure

Some side channels exist that this attestation does not mitigate:
- Hopper unencrypted NVLink (acknowledged; route confidential workloads to Blackwell B200/B300 when available).
- BAR0 register leakage (arxiv 2507.02770).
- Bimodal timing channels (batch-size leakage).
- GPUBreach (shared-timing).

These are listed in operator ToS and customer-facing docs. Network does not claim protection against silicon-undisclosed channels.

## Versioning

`version: u8 = 1` at TGE. Increment on any vendor matrix change. Backward-compat by tail-padding only.

## Open questions

- Whether to gate `dc-premium` on Blackwell-only operators once the Hopper NVLink issue is publicly catalogued. Default: market-discovered; expose `nvlink_encryption: bool` to customer routing filter.
