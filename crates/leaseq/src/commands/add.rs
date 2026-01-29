use anyhow::{Result, anyhow};
use std::process::Command;
use crate::commands::lease::{create_lease_quiet, CreateLeaseArgs};
use crate::commands::shell;

pub async fn run(slurm_args: Vec<String>) -> Result<()> {
    // 1. Create the Lease (Allocation)
    // We treat all provided arguments as sbatch passthrough arguments
    println!("Requesting new interactive lease allocation with args: {:?}", slurm_args);
    
    let args = CreateLeaseArgs {
        nodes: 1, // Default, can be overridden by sbatch_arg
        time: None,
        partition: None,
        qos: None,
        gpus_per_node: 0,
        account: None,
        sbatch_arg: slurm_args,
        wait: 0,
    };

    let result = create_lease_quiet(args).await?;
    let lease_id = result.job_id;
    println!("Lease allocated: {}", lease_id);

    // 2. Auto-connect to the interactive lease
    println!("Waiting for lease {} to start...", lease_id);
    wait_for_job_start(&lease_id).await?;
    
    // Automatically drop into shell in the newly allocated lease
    return shell::run(Some(lease_id), None).await;
}

async fn wait_for_job_start(job_id: &str) -> Result<()> {
    use std::time::{Duration, Instant};
    
    let start = Instant::now();
    let timeout = Duration::from_secs(300);
    let poll_interval = Duration::from_secs(2);

    loop {
        if start.elapsed() > timeout {
            return Err(anyhow!("Timeout waiting for lease to start"));
        }

        let output = Command::new("squeue")
            .args(["--job", job_id, "--noheader", "--format=%T"])
            .output()?;

        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if state == "RUNNING" {
            return Ok(());
        }
        if state.is_empty() {
             return Err(anyhow!("Lease {} lost (not in queue)", job_id));
        }
        
        tokio::time::sleep(poll_interval).await;
    }
}