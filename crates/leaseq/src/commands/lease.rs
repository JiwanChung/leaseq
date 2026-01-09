use clap::{Args, Subcommand};
use anyhow::{Result, Context};
use std::process::Command;
use std::io::Write;
use tempfile::NamedTempFile;
use leaseq_core::config;

#[derive(Subcommand)]
pub enum LeaseCommands {
    /// Create a new Slurm lease
    Create(CreateLeaseArgs),
    /// Release (cancel) a lease
    Release {
        lease_id: String,
    },
    /// List leases (from index)
    Ls,
}

#[derive(Args, Debug, Clone)]
pub struct CreateLeaseArgs {
    /// Number of nodes
    #[arg(long, default_value = "1")]
    pub nodes: u32,

    /// Time limit (e.g. 01:00:00). If not specified, uses cluster default (often unlimited).
    #[arg(long)]
    pub time: Option<String>,

    /// Partition
    #[arg(long)]
    pub partition: Option<String>,

    /// QoS (Quality of Service). If not specified, uses cluster default.
    #[arg(long)]
    pub qos: Option<String>,

    /// GPUs per node
    #[arg(long, default_value = "0")]
    pub gpus_per_node: u32,

    /// Account
    #[arg(long)]
    pub account: Option<String>,

    /// Additional sbatch args
    #[arg(long)]
    pub sbatch_arg: Vec<String>,

    /// Timeout in seconds to wait for job to start. If exceeded, job is cancelled. 0 = no wait.
    #[arg(long, default_value = "30")]
    pub wait: u64,
}

pub async fn run(command: LeaseCommands) -> Result<()> {
    match command {
        LeaseCommands::Create(args) => create_lease(args).await,
        LeaseCommands::Release { lease_id } => release_lease(lease_id).await,
        LeaseCommands::Ls => list_leases().await,
    }
}

pub async fn create_lease(args: CreateLeaseArgs) -> Result<()> {
    // 1. Check if sbatch is available
    if Command::new("sbatch").arg("--version").output().is_err() {
        return Err(anyhow::anyhow!("'sbatch' not found. Cannot create Slurm lease on this machine."));
    }

    // 2. Generate Keeper Script
    // We need to find the path to `leaseq-runner`. 
    // For now, assume it's in the same dir as the current executable or in PATH.
    let runner_bin = std::env::current_exe()?
        .parent().unwrap()
        .join("leaseq-runner");
        
    let runner_cmd = if runner_bin.exists() {
        runner_bin.to_string_lossy().to_string()
    } else {
        "leaseq-runner".to_string()
    };

    let mut script = String::new();
    script.push_str("#!/bin/bash\n");
    script.push_str(&format!("#SBATCH --nodes={}\n", args.nodes));
    if let Some(t) = &args.time {
        script.push_str(&format!("#SBATCH --time={}\n", t));
    }
    if let Some(p) = &args.partition {
        script.push_str(&format!("#SBATCH --partition={}\n", p));
    }
    if let Some(q) = &args.qos {
        script.push_str(&format!("#SBATCH --qos={}\n", q));
    }
    if let Some(a) = &args.account {
        script.push_str(&format!("#SBATCH --account={}\n", a));
    }
    if args.gpus_per_node > 0 {
        script.push_str(&format!("#SBATCH --gpus-per-node={}\n", args.gpus_per_node));
    }
    script.push_str("#SBATCH --job-name=leaseq\n");
    script.push_str("#SBATCH --output=leaseq-%j.log\n");
    
    for arg in &args.sbatch_arg {
        script.push_str(&format!("#SBATCH {}\n", arg));
    }

    script.push('\n');
    script.push_str("echo \"Starting leaseq runner on $SLURM_JOB_ID\"\n");
    // srun launches runner on all nodes
    script.push_str(&format!("srun {} --lease $SLURM_JOB_ID --node $(hostname)\n", runner_cmd));
    script.push_str("sleep 30\n"); // Grace period

    // 3. Write to temp file
    let mut temp = NamedTempFile::new()?;
    temp.write_all(script.as_bytes())?;
    
    // 4. Submit
    let output = Command::new("sbatch")
        .arg("--parsable")
        .arg(temp.path())
        .output()
        .context("Failed to execute sbatch")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("sbatch failed: {}", stderr));
    }

    let job_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    println!("Submitted Slurm job: {}", job_id);

    // Wait for job to start if requested
    if args.wait > 0 {
        println!("Waiting up to {}s for job to start...", args.wait);
        match wait_for_job_start(&job_id, args.wait).await {
            Ok(()) => {
                println!("Lease {} is now RUNNING", job_id);
            }
            Err(e) => {
                eprintln!("Timeout waiting for job to start: {}", e);
                eprintln!("Cancelling job {}...", job_id);
                let _ = Command::new("scancel").arg(&job_id).status();
                return Err(anyhow::anyhow!("Job did not start within {}s, cancelled.", args.wait));
            }
        }
    } else {
        println!("Lease created (not waiting for start): {}", job_id);
    }

    Ok(())
}

async fn wait_for_job_start(job_id: &str, timeout_secs: u64) -> Result<()> {
    use std::time::{Duration, Instant};

    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_secs(2);

    loop {
        if start.elapsed() > timeout {
            return Err(anyhow::anyhow!("Timeout after {}s", timeout_secs));
        }

        // Check job state with squeue
        let output = Command::new("squeue")
            .args(["--job", job_id, "--noheader", "--format=%T"])
            .output()
            .context("Failed to run squeue")?;

        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();

        match state.as_str() {
            "RUNNING" => return Ok(()),
            "PENDING" | "CONFIGURING" => {
                // Still waiting
                print!(".");
                std::io::Write::flush(&mut std::io::stdout())?;
            }
            "" => {
                // Job not found - might have completed already or failed
                return Err(anyhow::anyhow!("Job {} not found in queue", job_id));
            }
            other => {
                // FAILED, CANCELLED, etc.
                return Err(anyhow::anyhow!("Job entered unexpected state: {}", other));
            }
        }

        tokio::time::sleep(poll_interval).await;
    }
}

async fn release_lease(lease_id: String) -> Result<()> {
    if lease_id.starts_with("local:") {
        return Err(anyhow::anyhow!("Cannot release local lease via this command. Stop the runner process instead."));
    }
    
    let status = Command::new("scancel")
        .arg(&lease_id)
        .status()
        .context("Failed to run scancel")?;
        
    if status.success() {
        println!("Released lease {}", lease_id);
    } else {
        println!("Failed to release lease {}", lease_id);
    }
    Ok(())
}

async fn list_leases() -> Result<()> {
    // Read index or just list directories in runs/
    // Since we didn't implement the index yet, let's just list dirs in ~/.leaseq/runs
    let runs_dir = config::leaseq_home_dir().join("runs");
    if !runs_dir.exists() {
        println!("No leases found.");
        return Ok(())
    }
    
    for entry in std::fs::read_dir(runs_dir)? {
        let entry = entry?;
        if entry.path().is_dir() {
            println!("{}", entry.file_name().to_string_lossy());
        }
    }
    Ok(())
}
