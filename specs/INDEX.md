# RFC Index — Chain Tooling Specifications

Cross-team integration contracts ratified at end Q3 2026 (per plan §4). All RFCs versioned independently; SCALE on-chain encoding canonical; JSON off-chain. Breaking changes require all-leads sign-off + CI compat-test bump.

| # | RFC | Owner | Summary |
|---|---|---|---|
| [0001](./RFC-0001-receipt-format.md) | Signed Response Receipt Format | Verification Lead | Canonical per-inference receipt: operator-signed, includes `customer_nonce`, `attestation_report_hash`, `kernel_pack_hash`, log-prob sample. Unit of work measurement, dispute, BME settlement. SCALE on-chain, JSON off-chain, ed25519. |
| [0002](./RFC-0002-attestation-report.md) | Multi-Vendor TEE Attestation Report | Security Lead | Combined NVIDIA CC + Intel TDX + AMD SEV-SNP quotes hashed into `OnChainAttestation` in `pallet-attestation-registry`. Per-tier vendor matrix (red-team rule 8). Includes CRL (multisig-gated, sanctions/CVE fast-track). |
| [0003](./RFC-0003-heartbeat-schema.md) | Heartbeat Schema | Serving Lead | Off-chain heartbeat every 12s (capabilities, load, KV pressure, attestation freshness, watchdog) plus on-chain liveness anchor every epoch. Feeds gateway routing + `validator-watcher` anomaly detection. |
| [0004](./RFC-0004-batch-settlement.md) | Batch Settlement Format | Pallet Lead | Aggregates ~thousands of receipts into one extrinsic per epoch per gateway. Merkle-rooted, dispute window T+24h, slash on overcommit. Reduces 100M tx/day to ~280K. |
| [0005](./RFC-0005-slashing-extrinsic.md) | Slashing Extrinsic ABI | Pallet + Verification Leads | Per-detection slashing with fault codes (`WrongModel` 10%, `LogProbDrift` 2%, `FakeBurn` 50%, `DeviceCertCollision`/`SanctionsHit` 100%). Escrow not burn until T+28d; 7d dispute window; sortition arbitration panel; watcher bond ×10 on false claim. |
| [0006](./RFC-0006-sampling-randomness.md) | Commit-Reveal Sampling Randomness | Verification Lead | Per-epoch validator commit-reveal seeds the receipt sample. Stake-weighted Fisher-Yates over operators; ≥10% sample floor, per-tier ceiling up to 25% (edge). Defection → emission loss + permit revocation. |
| [0007](./RFC-0007-nonce-protocol.md) | Customer Nonce Anti-Replay | SDK + Pallet Leads | Customer-generated 256-bit nonce signed at request time; operator-side Bloom filter + 24h window; on-chain short-hash burn at settlement. Defense in depth at gateway, operator, chain. SDK helpers auto-generate. |
| [0008](./RFC-0008-oracle-feed.md) | Price Oracle Feed | Tokenomics + Pallet Leads | 4-source TWAP (Binance, Coinbase, UniV3-ETH, UniV4-local). 4–12h window first 18 mo post-TGE, 30 min after. 5% cross-source outlier clip; static fallback; `emergency_pause` multisig. Oracle pool ≥5 members, separate stake. |
| [0009](./RFC-0009-operator-registration.md) | Operator Registration Flow | Pallet + Compliance Leads | Registration extrinsic with coldkey/hotkey, attestation, tier, stake (USEFUL + USD-pegged ratchet per DECISIONS.md H9), geo region, /24 IP hash, sanctions-check proof. Device-cert collision across coldkeys → both slashed 100% (red-team rule 7). |
| [0010](./RFC-0010-rpc-endpoint-contract.md) | RPC Endpoint Provider Contract | Infra Lead + Foundation Ops | 3 independent providers (foundation + 2 partners) at TGE: Substrate JSON-RPC + Frontier EVM + WS subs + archive + receipt CDN. 99.9% uptime SLA; status page; operator daemon auto-failover within 2 min; embedded-validator fallback if all three down. |

## Cross-reference matrix

| RFC | Depends on / referenced by |
|---|---|
| 0001 | Referenced by: 0004 (Merkle leaves), 0005 (evidence), 0006 (sample target), 0007 (nonce field) |
| 0002 | Referenced by: 0001 (`attestation_report_hash`), 0003 (`AttestationFreshness`), 0005 (`SanctionsHit`, `DeviceCertCollision`), 0009 (registration validation) |
| 0003 | Depends on 0002 (freshness). Referenced by: 0004 (gateway routing assumes capability advertisement) |
| 0004 | Depends on: 0001 (receipts as leaves), 0007 (nonce burn at submit), 0008 (TWAP for burn:mint check). Referenced by: 0005 (`BatchOvercommit`, `FakeBurn`), 0006 (sample selection happens over batch receipts) |
| 0005 | Referenced by: 0006 (defection penalties), 0008 (`OracleManipulation` fault code), 0009 (collision auto-slash), runbooks 02, 04, 09 |
| 0006 | Depends on: 0001, 0004, 0005. Referenced by runbook 04 |
| 0007 | Referenced by: 0001 (`customer_nonce`), 0004 (burn at settlement). Referenced by runbook 09 |
| 0008 | Depends on: 0005 (oracle defection slashing). Referenced by: 0004 (settlement rate), 0009 (USD-pegged stake), runbook 03 |
| 0009 | Depends on: 0002, 0005, 0008. Referenced by registration flow + sanctions screener |
| 0010 | Operational only; no direct on-chain dependency. Referenced by runbook 08 |

## Open governance items

These appear in `DECISIONS.md`:

- **H9** — Operator stake currency at launch. Default: OROG with USD-pegged ratchet via governance every 30 days (locked into RFC-0008 and RFC-0009).
- RFCs surface additional open questions in their respective "Open questions" sections; all are governance-mutable behind 5-of-7 multisig + 14-d timelock unless flagged as runtime-upgrade-only.

## Compatibility

Each RFC has a `version: u8 = 1` field at TGE. Mixed-version tolerance is one runtime cycle. CI runs a compat-test suite that ensures any field-additive change is decodable by the prior version's struct.

Repository: `chain-tooling-rust/specs/`. Each RFC lives at its own path; this INDEX is the front door.
