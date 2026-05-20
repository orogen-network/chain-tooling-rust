//! Admin CLI helpers — chain-spec generation, slash-receipt verification, RFC validation.
//!
//! Mostly thin wrappers around chain RPC and on-disk artifacts. Designed so the binary in
//! `bin/mining-cli.rs` stays slim and most logic is library-testable.

use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChainSpec {
    pub name: String,
    pub id: String,
    pub chain_type: String,
    pub boot_nodes: Vec<String>,
    pub genesis: GenesisConfig,
    pub fork_blocks: Option<()>,
    pub bad_blocks: Option<()>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisConfig {
    pub initial_validators: Vec<String>,
    pub initial_operators: Vec<String>,
    pub bootstrap_cap_year1_bps: u32,
    pub bootstrap_cap_year2_bps: u32,
    pub elasticity_factor_ppm: u32,
    pub initial_treasury_balance: u128,
    pub initial_emission_pool_balance: u128,
}

pub fn default_mainnet_spec() -> ChainSpec {
    ChainSpec {
        name: "orogen-mainnet".to_string(),
        id: "orogen_mainnet".to_string(),
        chain_type: "Live".to_string(),
        boot_nodes: vec![],
        fork_blocks: None,
        bad_blocks: None,
        genesis: GenesisConfig {
            initial_validators: vec![],
            initial_operators: vec![],
            // Year-1 bootstrap cap = 8% (800 bps)
            bootstrap_cap_year1_bps: 800,
            // Year-2 cap = 4% (400 bps)
            bootstrap_cap_year2_bps: 400,
            // Elasticity factor = 1.0 = 1_000_000 ppm
            elasticity_factor_ppm: 1_000_000,
            initial_treasury_balance: 100_000_000_000_000_000_000_000, // 100M OROG × 10^18
            initial_emission_pool_balance: 400_000_000_000_000_000_000_000, // 400M OROG
        },
    }
}

pub fn default_testnet_spec() -> ChainSpec {
    ChainSpec {
        name: "orogen-forge".to_string(),
        id: "orogen_forge".to_string(),
        chain_type: "Live".to_string(),
        boot_nodes: vec![],
        fork_blocks: None,
        bad_blocks: None,
        genesis: GenesisConfig {
            initial_validators: vec![],
            initial_operators: vec![],
            // Forge testnet: lower thresholds, faster ratchet
            bootstrap_cap_year1_bps: 5000, // 50% — fast bootstrap of test economy
            bootstrap_cap_year2_bps: 2000,
            elasticity_factor_ppm: 1_500_000,
            initial_treasury_balance: 1_000_000_000_000_000_000_000_000,
            initial_emission_pool_balance: 4_000_000_000_000_000_000_000_000,
        },
    }
}

/// Compute a Blake2b-256 digest matching Substrate's `sp_core::blake2_256`
/// (which is a 32-byte Blake2b, NOT Blake2b-512 truncated).
///
/// This is what the runtime computes for content hashes (model manifests,
/// attestation reports, etc.). Off-chain tooling must match exactly to
/// re-derive the same on-chain ids.
pub fn blake2_256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Blake2bVar::new(32).expect("32 is a valid Blake2b output length");
    hasher.update(bytes);
    let mut out = [0u8; 32];
    hasher
        .finalize_variable(&mut out)
        .expect("output length matches configured length");
    out
}

/// Check a slash receipt's format and cosigner-count claims.
///
/// This intentionally does not perform cryptographic signature verification.
/// Callers must not treat a successful result as validator authorization.
#[derive(Debug, Serialize, Deserialize)]
pub struct SlashReceipt {
    pub slash_id: u64,
    pub operator_id: String,
    pub fault_code: String,
    pub severity_bps: u16,
    pub evidence_hash: String,
    pub validator_signatures: Vec<(String, String)>,
}

pub fn check_slash_format(receipt: &SlashReceipt) -> Result<(), String> {
    if !receipt.fault_code.chars().all(|c| c.is_alphanumeric()) {
        return Err(format!("invalid fault_code: {}", receipt.fault_code));
    }
    if receipt.severity_bps > 10_000 {
        return Err(format!("severity_bps > 10000: {}", receipt.severity_bps));
    }
    if !receipt.evidence_hash.starts_with("0x") || receipt.evidence_hash.len() != 66 {
        return Err(format!(
            "evidence_hash not 0x-prefixed H256: {}",
            receipt.evidence_hash
        ));
    }
    let need_sigs = match receipt.severity_bps {
        0..=50 => 1,
        51..=500 => 2,
        501..=1000 => 3,
        1001..=5000 => 3,
        _ => 5,
    };
    if receipt.validator_signatures.len() < need_sigs {
        return Err(format!(
            "insufficient cosigners: need {}, got {}",
            need_sigs,
            receipt.validator_signatures.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mainnet_spec_uses_8_percent_year1() {
        let spec = default_mainnet_spec();
        assert_eq!(spec.genesis.bootstrap_cap_year1_bps, 800);
        assert_eq!(spec.genesis.bootstrap_cap_year2_bps, 400);
    }

    #[test]
    fn default_testnet_spec_uses_50_percent_year1() {
        let spec = default_testnet_spec();
        assert_eq!(spec.genesis.bootstrap_cap_year1_bps, 5000);
    }

    #[test]
    fn blake2_256_is_deterministic_and_32_bytes() {
        let a = blake2_256(b"hello");
        let b = blake2_256(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    /// Known-answer test against `sp_core::blake2_256(b"")`. The empty-input
    /// digest of Blake2b-256 is the canonical reference vector and must
    /// match Substrate's `sp_core::blake2_256` implementation, otherwise
    /// on-chain ids computed off-chain will diverge.
    #[test]
    fn blake2_256_matches_substrate_known_answer() {
        let empty = blake2_256(b"");
        // Reference: sp_core::blake2_256(&[]) — Blake2b-256 of empty input.
        let expected =
            hex::decode("0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8")
                .unwrap();
        assert_eq!(empty.to_vec(), expected);

        // Reference vector for "abc" — Blake2b-256 (32-byte personalization /
        // output length) of "abc", matching `sp_core::blake2_256`.
        let abc = blake2_256(b"abc");
        let expected_abc =
            hex::decode("bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319")
                .unwrap();
        assert_eq!(abc.to_vec(), expected_abc);
    }

    #[test]
    fn slash_format_validates_severity_bound() {
        let mut r = SlashReceipt {
            slash_id: 1,
            operator_id: "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV".into(),
            fault_code: "WrongModel".into(),
            severity_bps: 11_000,
            evidence_hash: "0x".to_string() + &"01".repeat(32),
            validator_signatures: vec![],
        };
        assert!(check_slash_format(&r).is_err());
        r.severity_bps = 1000;
        // 1000 bps requires 3 signatures
        assert!(check_slash_format(&r).is_err());
        r.validator_signatures = vec![
            ("v1".into(), "sig1".into()),
            ("v2".into(), "sig2".into()),
            ("v3".into(), "sig3".into()),
        ];
        assert!(check_slash_format(&r).is_ok());
    }

    #[test]
    fn slash_format_rejects_bad_evidence_hash() {
        let r = SlashReceipt {
            slash_id: 1,
            operator_id: "x".into(),
            fault_code: "WrongModel".into(),
            severity_bps: 100,
            evidence_hash: "not-hex".into(),
            validator_signatures: vec![("v1".into(), "sig".into())],
        };
        assert!(check_slash_format(&r).is_err());
    }

    #[test]
    fn slash_format_rejects_nonalphanumeric_fault_code() {
        let r = SlashReceipt {
            slash_id: 1,
            operator_id: "x".into(),
            fault_code: "Wrong-Model".into(),
            severity_bps: 100,
            evidence_hash: "0x".to_string() + &"01".repeat(32),
            validator_signatures: vec![("v1".into(), "sig".into())],
        };
        assert!(check_slash_format(&r).is_err());
    }
}
