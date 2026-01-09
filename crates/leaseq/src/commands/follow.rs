use anyhow::Result;
use leaseq_core::{config, fs as lfs, models};
use std::path::{Path, PathBuf};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::time::Duration;

pub async fn run(
    task: Option<String>,
    lease: Option<String>,
    node: Option<String>,
    stderr: bool,
) -> Result<()> {
    let lease_id = lease.unwrap_or_else(config::local_lease_id);

    let root = if lease_id.starts_with("local:") {
        config::runtime_dir().join(&lease_id)
    } else {
        config::leaseq_home_dir().join("runs").join(&lease_id)
    };

    // Determine which task to follow
    let task_id = if let Some(t) = task {
        t
    } else {
        // Find the currently running task
        find_running_task(&root, node.as_deref())?
    };

    let log_path = if stderr {
        root.join("logs").join(format!("{}.err", task_id))
    } else {
        root.join("logs").join(format!("{}.out", task_id))
    };

    eprintln!("Following {} (Ctrl+C to stop)", log_path.display());

    // Tail follow the file
    tail_follow(&log_path).await
}

fn find_running_task(root: &Path, node_filter: Option<&str>) -> Result<String> {
    let claimed_dir = root.join("claimed");

    if !claimed_dir.exists() {
        return Err(anyhow::anyhow!("No running tasks found. Specify --task explicitly."));
    }

    let mut running_tasks = Vec::new();

    for entry in std::fs::read_dir(&claimed_dir)? {
        let entry = entry?;
        if entry.path().is_dir() {
            let node_name = entry.file_name().to_string_lossy().into_owned();

            // Apply node filter if specified
            if let Some(filter) = node_filter {
                if node_name != filter {
                    continue;
                }
            }

            if let Ok(files) = lfs::list_files_sorted(entry.path()) {
                for f in files {
                    if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&f) {
                        running_tasks.push((spec.task_id, node_name.clone()));
                    }
                }
            }
        }
    }

    match running_tasks.len() {
        0 => Err(anyhow::anyhow!("No running tasks found. Specify --task explicitly.")),
        1 => Ok(running_tasks[0].0.clone()),
        _ => {
            eprintln!("Multiple running tasks found:");
            for (id, node) in &running_tasks {
                eprintln!("  {} on {}", id, node);
            }
            Err(anyhow::anyhow!("Please specify --task or --node to select one."))
        }
    }
}

async fn tail_follow(path: &PathBuf) -> Result<()> {
    let poll_interval = Duration::from_millis(250);

    // Wait for file to exist
    while !path.exists() {
        tokio::time::sleep(poll_interval).await;
    }

    let mut file = std::fs::File::open(path)?;

    // Start from current end
    let mut pos = file.seek(SeekFrom::End(0))?;

    let mut buffer = vec![0u8; 4096];

    loop {
        // Check for new data
        let current_len = file.metadata()?.len();

        if current_len > pos {
            file.seek(SeekFrom::Start(pos))?;

            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                io::stdout().write_all(&buffer[..n])?;
                io::stdout().flush()?;
                pos += n as u64;
            }
        } else if current_len < pos {
            // File was truncated, start over
            pos = 0;
            file.seek(SeekFrom::Start(0))?;
        }

        tokio::time::sleep(poll_interval).await;
    }
}
