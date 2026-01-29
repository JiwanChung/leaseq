use anyhow::Result;
use leaseq_core::config;
use std::process::Command;
use std::os::unix::process::CommandExt; // For exec

pub async fn run(lease: Option<String>, node: Option<String>) -> Result<()> {
    // 1. Resolve Lease
    let lease_id = lease.unwrap_or_else(config::local_lease_id);
    
    // Check if lease is local or slurm
    if lease_id.starts_with("local:") {
        // Local Shell
        // If we are on the same machine, just exec shell.
        // If lease_id implies a specific local lease (e.g. local:remotehost?), we assume local:hostname is THIS machine.
        
        let hostname = hostname::get()?.to_string_lossy().into_owned();
        if lease_id != format!("local:{}", hostname) && lease_id != "local:localhost" {
             // If local lease is for another host (e.g. over NFS mount but different machine?), warn.
             // But usually local lease is strictly local.
        }

        println!("Starting shell in local lease {}...", lease_id);
        
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        
        let err = Command::new(&shell)
            .exec();
            
        return Err(anyhow::Error::from(err).context("Failed to exec shell"));
    } else {
        // Slurm Shell
        // We assume lease_id is the Job ID.
        println!("Starting interactive shell in Slurm lease {}...", lease_id);
        
        let mut cmd = Command::new("srun");
        cmd.arg("--jobid").arg(&lease_id);
        
        if let Some(n) = node {
            cmd.arg("--nodelist").arg(n);
        }
        
        cmd.arg("--pty");
        cmd.arg("bash"); // Default to bash on cluster
        
        let err = cmd.exec();
        
        return Err(anyhow::Error::from(err).context("Failed to exec srun"));
    }
}