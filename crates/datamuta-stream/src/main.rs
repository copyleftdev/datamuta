#![forbid(unsafe_code)]
mod operator;
use clap::{Parser, Subcommand, ValueEnum};
use datamuta_kernel::Fingerprint;
use datamuta_stream::{self as stream, Plan, Result, publisher};
use std::path::PathBuf;
#[derive(Parser)]
#[command(
    version,
    about = "Experimental sealed typed snapshot streaming; exact-plan approval required"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Discover PostgreSQL schemas and validate an explicit field mapping.
    Configure {
        #[arg(long)]
        source_endpoint: PathBuf,
        #[arg(long)]
        target_endpoint: PathBuf,
        #[arg(long)]
        source_table: String,
        #[arg(long)]
        target_table: String,
        #[arg(long)]
        mapping: PathBuf,
        #[arg(long, default_value_t = 268435456)]
        spill_bytes: u64,
        #[arg(long)]
        out: PathBuf,
    },
    /// Suggest a draft mapping through a saved JEV exchange; grants no authorization.
    SuggestMapping {
        #[command(subcommand)]
        step: Suggest,
    },
    /// Inspect the plan and sealed manifest offline; grants no authorization.
    Inspect {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        spool: PathBuf,
    },
    /// Compare every PostgreSQL destination value and receipt with the sealed plan.
    Verify {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        spool: PathBuf,
        /// Save a new JSON evidence file; an existing path is never overwritten.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Capture a source snapshot into a new sealed spool and plan.
    Capture {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        spool: PathBuf,
        #[arg(long)]
        plan: PathBuf,
        #[arg(long, hide = true)]
        stop_after_rows: Option<u64>,
    },
    /// Run or resume this exact approved plan from native committed progress.
    Run {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        spool: PathBuf,
        #[arg(long)]
        approved_plan_sha256: String,
        #[arg(long, hide = true)]
        fault: Option<Fault>,
        #[arg(long, hide = true, default_value_t = 0)]
        partition: u64,
    },
    /// Read native committed progress without scanning completed payloads.
    Status {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        spool: PathBuf,
    },
    /// Check receipt history; use verify for destination values.
    Audit {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        spool: PathBuf,
    },
}
#[derive(clap::Args)]
struct Relations {
    #[arg(long)]
    source_endpoint: PathBuf,
    #[arg(long)]
    target_endpoint: PathBuf,
    #[arg(long)]
    source_table: String,
    #[arg(long)]
    target_table: String,
    /// Human-written meaning for every source and target field.
    #[arg(long)]
    evidence: PathBuf,
}
#[derive(Subcommand)]
enum Suggest {
    /// Enumerate valid mappings; write a JEV request, or the only candidate.
    Prepare {
        #[command(flatten)]
        relations: Relations,
        #[arg(long)]
        request_out: PathBuf,
        #[arg(long)]
        mapping_out: PathBuf,
    },
    /// Prepare, send to JEV once, save the reply and admit it (key in DATAMUTA_JEV_API_KEY).
    Ask {
        #[command(flatten)]
        relations: Relations,
        #[arg(long, default_value = stream::suggest::DEFAULT_ENDPOINT)]
        endpoint: String,
        #[arg(long)]
        request_out: PathBuf,
        #[arg(long)]
        response_out: PathBuf,
        #[arg(long)]
        mapping_out: PathBuf,
        #[arg(long)]
        provenance_out: PathBuf,
    },
    /// Admit a saved JEV response into a draft mapping, or require review (exit 3).
    Admit {
        #[command(flatten)]
        relations: Relations,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        response: PathBuf,
        #[arg(long)]
        mapping_out: PathBuf,
        #[arg(long)]
        provenance_out: PathBuf,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum Fault {
    BeforeCommit,
    AfterCommit,
}
fn rss() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find(|s| s.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
async fn execute(command: Command) -> Result<serde_json::Value> {
    let started = std::time::Instant::now();
    let mut evidence = None;
    let mut value = match command {
        Command::Configure {
            source_endpoint,
            target_endpoint,
            source_table,
            target_table,
            mapping,
            spill_bytes,
            out,
        } => {
            let config = operator::configure(
                &source_endpoint,
                &target_endpoint,
                &source_table,
                &target_table,
                &mapping,
                spill_bytes,
            )
            .await?;
            stream::save(&out, &config)?;
            serde_json::json!({"status":"configured","config":out,"authorization":"not granted"})
        }
        Command::SuggestMapping { step } => match step {
            Suggest::Prepare {
                relations: r,
                request_out,
                mapping_out,
            } => {
                operator::suggest_prepare(
                    (&r.source_endpoint, &r.target_endpoint),
                    (&r.source_table, &r.target_table),
                    &r.evidence,
                    &request_out,
                    &mapping_out,
                )
                .await?
            }
            Suggest::Ask {
                relations: r,
                endpoint,
                request_out,
                response_out,
                mapping_out,
                provenance_out,
            } => {
                operator::suggest_ask(
                    (&r.source_endpoint, &r.target_endpoint),
                    (&r.source_table, &r.target_table),
                    &r.evidence,
                    &endpoint,
                    [&request_out, &response_out, &mapping_out, &provenance_out],
                )
                .await?
            }
            Suggest::Admit {
                relations: r,
                request,
                response,
                mapping_out,
                provenance_out,
            } => {
                operator::suggest_admit(
                    (&r.source_endpoint, &r.target_endpoint),
                    (&r.source_table, &r.target_table),
                    &r.evidence,
                    &request,
                    &response,
                    &mapping_out,
                    &provenance_out,
                )
                .await?
            }
        },
        Command::Inspect { plan, spool } => operator::inspect(&stream::load(&plan)?, &spool)?,
        Command::Verify { plan, spool, out } => {
            if out.as_ref().is_some_and(|p| p.symlink_metadata().is_ok()) {
                return Err(std::io::Error::other("evidence output already exists").into());
            }
            evidence = out;
            operator::verify(&stream::load(&plan)?, &spool).await?
        }
        Command::Capture {
            config,
            spool,
            plan,
            stop_after_rows,
        } => {
            let config = stream::load(&config)?;
            let p = stream::capture(config, &spool, |n| {
                if stop_after_rows == Some(n) {
                    std::process::exit(86);
                }
                Ok(())
            })
            .await?;
            stream::save(&plan, &p)?;
            serde_json::json!({"status":"pass","plan_sha256":p.fingerprint()?,"manifest":p.snapshot})
        }
        Command::Run {
            plan,
            spool,
            approved_plan_sha256,
            fault,
            partition,
        } => {
            let plan: Plan = stream::load(&plan)?;
            let approved = Fingerprint::try_from(approved_plan_sha256)?;
            serde_json::to_value(
                publisher::run(&plan, &spool, &approved, |at, n| {
                    let hit = matches!(
                        (fault, at),
                        (Some(Fault::BeforeCommit), publisher::Boundary::BeforeCommit)
                            | (Some(Fault::AfterCommit), publisher::Boundary::AfterCommit)
                    );
                    if hit && n == partition {
                        std::process::exit(86);
                    }
                    Ok(())
                })
                .await?,
            )?
        }
        Command::Status { plan, spool } => {
            serde_json::to_value(publisher::status(&stream::load(&plan)?, &spool).await?)?
        }
        Command::Audit { plan, spool } => {
            serde_json::to_value(publisher::audit(&stream::load(&plan)?, &spool).await?)?
        }
    };
    value["peak_rss_kib"] = serde_json::json!(rss());
    value["process_seconds"] = serde_json::json!(started.elapsed().as_secs_f64());
    if let Some(path) = evidence {
        stream::save(&path, &value)?;
    }
    Ok(value)
}
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let command = Cli::parse().command;
    match tokio::time::timeout(std::time::Duration::from_secs(300), execute(command)).await {
        Ok(Ok(value)) => {
            println!("{value}");
            if value["status"] == "review_required" {
                std::process::exit(3);
            }
        }
        Ok(Err(e)) => {
            eprintln!("datamuta-stream: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("datamuta-stream: execution deadline");
            std::process::exit(1);
        }
    }
}
