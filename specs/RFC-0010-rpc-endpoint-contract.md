# RFC-0010 — RPC Endpoint Provider Contract

**Status:** Draft — ratification target end Q3 2026
**Owner:** Infra Lead + Foundation Ops
**Consumers:** `chain-node`, `gateway-router`, `worker-control-plane`, `wallet-sdk-core`, `customer-sdk-{py,ts}`, `explorer-web`, `status-page`
**Cross-cuts:** RFC-0003 (operator heartbeats RPC), RFC-0004 (batch submission RPC), RFC-0008 (oracle submission RPC)

## Goal

Ensure no single party can de-platform the network at the RPC layer. Targets red-team rule 12: at TGE, 3 independent providers run public Substrate JSON-RPC + Frontier EVM JSON-RPC endpoints with published SLA on a foundation-neutral status page.

## Provider topology at launch

| Provider | Operator | Region | Role |
|---|---|---|---|
| `rpc.mining.network` | Foundation-operated | US-East + EU-West dual | Reference (foundation runs 1) |
| `rpc.figment-class.mining.network` | Partner A (Figment-class) | NA + APAC | Independent |
| `rpc.allnodes-class.mining.network` | Partner B (Allnodes-class) | EU + APAC | Independent |

"Independent" means: separate legal entity, separate infrastructure account, separate Layer-1 hosting (not all on AWS). Foundation cannot revoke a partner's keys; partner cannot revoke foundation's.

The hostnames above are templates — exact provider names finalized at partner-onboarding.

## Required surface

Each provider commits to the full surface:

### Substrate JSON-RPC (mandatory)
- `chain_*`, `state_*`, `system_*`, `payment_*`, `author_*`, `rpc_methods`
- Runtime API: `Metadata`, `Core`, all pallet APIs in `runtime-mainnet`
- Custom RPC methods registered by pallets: `oracle_*`, `attestation_*`, `nonce_*`, `slashing_*` (these resolve to read-only chain state inspectors)

### Frontier EVM JSON-RPC (mandatory)
- `eth_*` standard methods (Metamask-compatible subset)
- `net_*`, `web3_*`
- `eth_subscribe` (newHeads, logs, newPendingTransactions)

### WebSocket subscriptions (mandatory)
- All chain subscriptions: `chain_subscribeNewHeads`, `chain_subscribeFinalizedHeads`, `state_subscribeStorage`
- All EVM subscriptions

### Archive node (mandatory)
- Full state at every block since genesis. No pruning. Required for explorer + dispute reconstruction.

### Off-chain CDN (mandatory)
- Serve receipt blobs for any `(batch_id, receipt_hash)` referenced by a batch the provider witnessed within the last 90 days.

## Method support matrix (excerpt)

| Method | Foundation | Partner A | Partner B | Notes |
|---|---|---|---|---|
| `chain_getBlock` | Y | Y | Y | |
| `state_call` | Y | Y | Y | |
| `state_getStorage` | Y | Y | Y | |
| `state_subscribeStorage` | Y | Y | Y | WS |
| `author_submitExtrinsic` | Y | Y | Y | Rate-limited per-IP |
| `eth_call` | Y | Y | Y | |
| `eth_estimateGas` | Y | Y | Y | |
| `eth_sendRawTransaction` | Y | Y | Y | Rate-limited |
| `eth_subscribe` | Y | Y | Y | WS |
| `oracle_getCurrentTwap` | Y | Y | Y | Pallet RPC |
| `attestation_isRevoked` | Y | Y | Y | Pallet RPC |
| `nonce_isBurned` | Y | Y | Y | Pallet RPC |
| `mining_replayReceipt` | Y | Optional | Optional | Heavy; foundation always supports |

## SLA targets

| Metric | Target | Measurement window | Penalty for miss |
|---|---|---|---|
| Uptime | 99.9% | Rolling 30 days | Demerit; 3 demerits in 90d → provider replaced |
| p99 query latency (cached read) | <500 ms | Rolling 24h | Public status flag |
| p99 query latency (state_call) | <2 s | Rolling 24h | Public status flag |
| WebSocket pub/sub delivery | <1 s | Rolling 24h | Public status flag |
| Block-tip freshness | within 3 blocks of chain head | Real-time | Status auto-marks degraded |
| Archive completeness | 100% | Audit monthly | Replacement on miss |
| CDN receipt availability | 99.5% | Rolling 30d | Demerit |
| Incident response | acknowledged <15 min for P0 | Per-incident | Demerit |

The `status-page` publishes uptime % and live latency per provider; greenwashing is impossible because the same data is verifiable by anyone querying the endpoint.

## Rate limits

Per-IP defaults (provider may relax for authenticated keys):

| Endpoint class | Limit |
|---|---|
| Read RPC (HTTP) | 100 req/s, 10K req/min |
| Read RPC (WebSocket subs) | 100 active subs/connection |
| Write RPC (`author_submitExtrinsic`, `eth_sendRawTransaction`) | 10 req/s |
| Heavy RPC (`mining_replayReceipt`) | 1 req/s per key |
| Archive deep-state queries | 10 req/s |

CORS:
- Mainnet endpoints: `Access-Control-Allow-Origin: *` for read-only methods; write endpoints restricted to a documented allowlist of frontend origins; non-origin'd POST requests always allowed (CLI/SDK).
- Testnet endpoints: `Access-Control-Allow-Origin: *` everywhere.

## Operator-daemon fallback logic

```python
# pseudocode — implemented in worker-control-plane
def rpc_client():
    primary, secondary, tertiary = load_rpc_pool()
    cur = primary
    while True:
        try:
            ws = connect(cur, timeout=2)
            while True:
                if last_block_age(ws) > 120:        # 2 minutes
                    raise StaleRpc(cur)
                yield ws
        except (StaleRpc, ConnError):
            cur = next_after(cur, [primary, secondary, tertiary])
            if cur is primary:                       # we've cycled through all
                spawn_local_validator_node_cold_sync(target_minutes=60)
                return local_ws
```

Targets:
- Primary RPC failover within 2 minutes of unresponsiveness.
- Tertiary failover within 4 minutes.
- All-three-down → operator runs its own validator node with 1h cold-sync target (uses pinned snapshots from `weight-cdn-pinner`).

## Mainnet vs. testnet endpoints

| Network | Hostnames | Notes |
|---|---|---|
| Mainnet | `rpc.mining.network`, `wss.mining.network`, `evm.mining.network` | Behind multi-region anycast |
| Testnet (Forge) | `rpc.forge.mining.network`, etc. | Faucet integrated |
| Testnet (Forge-Stake) | `rpc.forge-stake.mining.network` | KYC'd, incentivized |
| Testnet (Forge-Adversarial) | `rpc.forge-adv.mining.network` | Permissionless attack net |
| Shadowfork | `rpc.shadow.mining.network` | Pre-TGE final check; not public |

DNS records owned by foundation; SLA contracts include "right to transfer DNS to a successor provider on 30-d notice" — prevents provider lock-in.

## Partner SLA contract template (key clauses)

1. **Term.** 24 months initial, 12-month renewals.
2. **Compensation.** Per-month base fee + per-million-request bonus; bonuses pro-rated by SLA achievement.
3. **Right to audit.** Foundation may inspect provider logs (subject to GDPR redaction) and synthetic monitoring data.
4. **Method coverage attestation.** Partner publishes monthly attestation that all mandatory methods return correct results (signed by partner key).
5. **Key custody.** Provider holds operational keys; no slashing exposure (read/archive providers are not consensus validators).
6. **Termination triggers.** Three demerits in 90 days; material breach (intentional method removal); regulatory action against provider entity.
7. **Wind-down.** 60-day handoff to successor provider; provider continues service through handoff.
8. **Independence covenant.** Provider may not be acquired by foundation or any C-corp affiliate during the term.
9. **Transparency.** Provider's status page is publicly readable; uptime claims are independently measurable.
10. **Indemnification.** Mutual indemnity for negligence; provider not liable for chain-level malfunctions.

## Status page

`status-page` aggregates:
- Provider uptime % (live).
- p99 query latency per provider (live).
- WebSocket lag per provider (live).
- Block-tip freshness per provider (live).
- Archive completeness audit results (monthly).
- Incident log with timestamped impact + resolution.
- Demerit log (visible).
- Method support matrix snapshot.

`status-page` is foundation-operated but the data is queryable directly from each provider; status-page just aggregates and graphs.

## DDoS / abuse handling

Each provider commits to:
- Cloudflare-class WAF in front of HTTP endpoints.
- IP-rate-limiting + per-API-key rate-limiting at edge.
- Anycast routing for the WebSocket fleet.
- Failover within the provider's own pool independent of the cross-provider failover.

If an attack overwhelms a provider, the daemon failover (above) routes operators to remaining providers. If 2 of 3 providers degrade simultaneously (active attack), `chain-node` daemons fall back to running embedded validator nodes; the network keeps producing blocks because validators run on a separate private peer network not exposed via public RPC.

## Versioning

`version` not encoded on-chain (this is operational, not on-chain state). Method support matrix versioned in `chain-tooling-rust/specs/rpc-matrix.toml` (added in companion PR).

## Open questions

- Whether to require a 4th provider before mainnet (defense in depth). Default: 3 at TGE, target 5 by end of Year 1.
- Whether to publish provider keys on-chain so they can sign their own status attestations verifiably. Default: yes, registered in `pallet-treasury-ext::RpcProviders` map with rotation timelock.
