//! `mining-cli` — administrative CLI for the Orogen chain.
//!
//!     mining-cli chain-spec generate --network mainnet|forge --output spec.json
//!     mining-cli chain-spec print --network mainnet
//!     mining-cli slash verify --file receipt.json
//!     mining-cli rfc list
//!     mining-cli rfc check --spec-dir specs/

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use mining_cli::{default_mainnet_spec, default_testnet_spec, verify_slash_format, SlashReceipt};

#[derive(Parser)]
#[command(
    name = "mining-cli",
    version,
    about = "Admin CLI for the Orogen chain"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Chain spec generation + inspection.
    ChainSpec {
        #[command(subcommand)]
        op: ChainSpecOp,
    },
    /// Slash receipt utilities.
    Slash {
        #[command(subcommand)]
        op: SlashOp,
    },
    /// RFC spec listing + validation.
    Rfc {
        #[command(subcommand)]
        op: RfcOp,
    },
}

#[derive(Subcommand)]
enum ChainSpecOp {
    /// Print the default spec for a network.
    Print {
        #[arg(long, value_enum, default_value_t = Network::Mainnet)]
        network: Network,
    },
    /// Generate spec to a file.
    Generate {
        #[arg(long, value_enum, default_value_t = Network::Mainnet)]
        network: Network,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum SlashOp {
    /// Verify the format/cosigner-count of a slash receipt file.
    Verify {
        #[arg(long)]
        file: PathBuf,
    },
}

#[derive(Subcommand)]
enum RfcOp {
    /// List all RFCs in the specs directory.
    List {
        #[arg(long, default_value = "specs")]
        spec_dir: PathBuf,
    },
    /// Validate all RFCs are well-formed (filename pattern + first H1).
    Check {
        #[arg(long, default_value = "specs")]
        spec_dir: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Network {
    Mainnet,
    Forge,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::ChainSpec { op } => match op {
            ChainSpecOp::Print { network } => {
                let spec = match network {
                    Network::Mainnet => default_mainnet_spec(),
                    Network::Forge => default_testnet_spec(),
                };
                println!("{}", serde_json::to_string_pretty(&spec)?);
            }
            ChainSpecOp::Generate { network, output } => {
                let spec = match network {
                    Network::Mainnet => default_mainnet_spec(),
                    Network::Forge => default_testnet_spec(),
                };
                fs::write(&output, serde_json::to_string_pretty(&spec)?)
                    .with_context(|| format!("writing {}", output.display()))?;
                println!("wrote {}", output.display());
            }
        },
        Cmd::Slash { op } => match op {
            SlashOp::Verify { file } => {
                let bytes = fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
                let receipt: SlashReceipt = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing {}", file.display()))?;
                verify_slash_format(&receipt).map_err(|e| anyhow!(e))?;
                println!("ok");
            }
        },
        Cmd::Rfc { op } => match op {
            RfcOp::List { spec_dir } => {
                let mut entries: Vec<_> = fs::read_dir(&spec_dir)?
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("RFC-")
                    })
                    .collect();
                entries.sort_by_key(|e| e.file_name());
                for e in entries {
                    println!("{}", e.file_name().to_string_lossy());
                }
            }
            RfcOp::Check { spec_dir } => {
                let mut count = 0;
                for entry in fs::read_dir(&spec_dir)? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !name.starts_with("RFC-") || !name.ends_with(".md") {
                        continue;
                    }
                    let body = fs::read_to_string(entry.path())?;
                    let first_h1 = body.lines().find(|l| l.starts_with("# "));
                    if first_h1.is_none() {
                        return Err(anyhow!("{}: missing top-level H1", name));
                    }
                    count += 1;
                }
                println!("ok: {} RFCs validated", count);
            }
        },
    }
    Ok(())
}
