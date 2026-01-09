use anyhow::{Result, Context};
use leaseq_core::config;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn pid_file() -> PathBuf {
    config::runtime_dir().join("daemon.pid")
}

fn log_file() -> PathBuf {
    config::runtime_dir().join("daemon.log")
}

pub async fn start() -> Result<()> {
    // Check if already running
    if let Some(pid) = read_pid() {
        if is_process_running(pid) {
            println!("Daemon already running (PID {})", pid);
            return Ok(());
        }
    }

    let lease_id = config::local_lease_id();
    let root = config::runtime_dir().join(&lease_id);

    // Ensure directories exist
    fs::create_dir_all(&root)?;

    // Find the runner binary
    let runner_bin = find_runner_binary()?;

    // Start the runner
    let log = fs::File::create(log_file())?;

    let child = Command::new(&runner_bin)
        .arg("--lease")
        .arg(&lease_id)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
        .context("Failed to start runner")?;

    let pid = child.id();

    // Write PID file
    fs::write(pid_file(), pid.to_string())?;

    println!("Started daemon (PID {})", pid);
    println!("Lease: {}", lease_id);
    println!("Log: {}", log_file().display());

    Ok(())
}

pub async fn stop() -> Result<()> {
    let pid = read_pid();

    match pid {
        Some(pid) if is_process_running(pid) => {
            // Send SIGTERM
            #[cfg(unix)]
            {
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            }
            #[cfg(not(unix))]
            {
                // On non-Unix, try taskkill
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .status();
            }

            // Wait a bit and check
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            if !is_process_running(pid) {
                fs::remove_file(pid_file()).ok();
                println!("Stopped daemon (PID {})", pid);
            } else {
                println!("Sent SIGTERM to daemon (PID {}), may still be stopping...", pid);
            }
        }
        Some(_) => {
            fs::remove_file(pid_file()).ok();
            println!("Daemon was not running (stale PID file removed)");
        }
        None => {
            println!("Daemon is not running");
        }
    }

    Ok(())
}

pub async fn status() -> Result<()> {
    let lease_id = config::local_lease_id();
    let root = config::runtime_dir().join(&lease_id);

    println!("Local Lease: {}", lease_id);
    println!("Runtime Dir: {}", root.display());

    match read_pid() {
        Some(pid) if is_process_running(pid) => {
            println!("Daemon: RUNNING (PID {})", pid);
        }
        Some(pid) => {
            println!("Daemon: NOT RUNNING (stale PID {} in file)", pid);
        }
        None => {
            println!("Daemon: NOT RUNNING");
        }
    }

    // Check heartbeat
    let hb_dir = root.join("hb");
    if hb_dir.exists() {
        for entry in fs::read_dir(&hb_dir)? {
            let entry = entry?;
            if let Ok(content) = fs::read_to_string(entry.path()) {
                if let Ok(hb) = serde_json::from_str::<leaseq_core::models::Heartbeat>(&content) {
                    let age = (time::OffsetDateTime::now_utc() - hb.ts).as_seconds_f64();
                    let status = if age > 60.0 { "STALE" } else { "OK" };
                    println!(
                        "Runner {}: {} (heartbeat {:.0}s ago)",
                        hb.node, status, age
                    );
                }
            }
        }
    }

    Ok(())
}

fn read_pid() -> Option<u32> {
    fs::read_to_string(pid_file())
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill with signal 0 checks if process exists
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On Windows, try to open the process
        false // Simplified; would need winapi for proper check
    }
}

fn find_runner_binary() -> Result<PathBuf> {
    // Check next to current exe
    if let Ok(exe) = std::env::current_exe() {
        let runner = exe.parent().unwrap().join("leaseq-runner");
        if runner.exists() {
            return Ok(runner);
        }
    }

    // Check in PATH
    if let Ok(output) = Command::new("which").arg("leaseq-runner").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }

    Err(anyhow::anyhow!(
        "leaseq-runner not found. Build it with 'cargo build -p leaseq-runner'"
    ))
}
