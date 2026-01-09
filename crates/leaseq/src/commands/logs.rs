use anyhow::{Result, Context};
use leaseq_core::config;
use std::path::{Path, PathBuf};

pub async fn run(task: String, lease: Option<String>, stderr: bool, tail: Option<usize>) -> Result<()> {
    let lease_id = lease.unwrap_or_else(config::local_lease_id);

    let root = if lease_id.starts_with("local:") {
        config::runtime_dir().join(&lease_id)
    } else {
        config::leaseq_home_dir().join("runs").join(&lease_id)
    };

    let log_path = if stderr {
        root.join("logs").join(format!("{}.err", task))
    } else {
        root.join("logs").join(format!("{}.out", task))
    };

    if !log_path.exists() {
        // Try to find task by partial ID
        let found = find_task_log(&root, &task, stderr)?;
        if let Some(path) = found {
            print_log(&path, tail)?;
        } else {
            eprintln!("Log file not found: {}", log_path.display());
            eprintln!("Task {} may not exist or hasn't produced output yet.", task);
        }
        return Ok(());
    }

    print_log(&log_path, tail)
}

fn find_task_log(root: &Path, task_prefix: &str, stderr: bool) -> Result<Option<PathBuf>> {
    let logs_dir = root.join("logs");
    if !logs_dir.exists() {
        return Ok(None);
    }

    let ext = if stderr { ".err" } else { ".out" };

    for entry in std::fs::read_dir(&logs_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(task_prefix) && name.ends_with(ext) {
            return Ok(Some(entry.path()));
        }
    }

    Ok(None)
}

fn print_log(path: &PathBuf, tail: Option<usize>) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .context(format!("Failed to read {}", path.display()))?;

    if let Some(n) = tail {
        let lines: Vec<&str> = content.lines().collect();
        let start = if lines.len() > n { lines.len() - n } else { 0 };
        for line in &lines[start..] {
            println!("{}", line);
        }
    } else {
        print!("{}", content);
    }

    Ok(())
}
