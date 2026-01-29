use anyhow::Result;
use leaseq_core::{config, fs as lfs, models};
use uuid::Uuid;
use std::path::Path;

pub async fn run(task: String, lease: Option<String>) -> Result<()> {
    let lease_id = lease.unwrap_or_else(config::local_lease_id);

    let root = if lease_id.starts_with("local:") {
        config::runtime_dir().join(&lease_id)
    } else {
        config::leaseq_home_dir().join("runs").join(&lease_id)
    };

    // Find the task and determine which node it's on
    let (node, task_state) = find_task(&root, &task)?;

    match task_state.as_str() {
        "PENDING" => {
            cancel_pending_task(&root, &task, &node)?;
            println!("Cancelled pending task {} on {}", task, node);
        }
        "RUNNING" => {
            cancel_running_task(&root, &task, &node)?;
            println!("Sent cancel request for running task {} on {}", task, node);
            println!("Runner will terminate the task on next check.");
        }
        "DONE" | "FAILED" => {
            println!("Task {} has already completed (state: {})", task, task_state);
        }
        _ => {
            println!("Task {} in unknown state: {}", task, task_state);
        }
    }

    Ok(())
}

fn find_task(root: &Path, task_id: &str) -> Result<(String, String)> {
    // Check inbox (pending)
    let inbox_dir = root.join("inbox");
    if inbox_dir.exists() {
        for entry in std::fs::read_dir(&inbox_dir)? {
            let entry = entry?;
            if entry.path().is_dir() {
                let node = entry.file_name().to_string_lossy().into_owned();
                for task_file in lfs::list_files_sorted(entry.path())? {
                    if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&task_file) {
                        if spec.task_id == task_id || spec.task_id.starts_with(task_id) {
                            return Ok((node, "PENDING".to_string()));
                        }
                    }
                }
            }
        }
    }

    // Check claimed (running)
    let claimed_dir = root.join("claimed");
    if claimed_dir.exists() {
        for entry in std::fs::read_dir(&claimed_dir)? {
            let entry = entry?;
            if entry.path().is_dir() {
                let node = entry.file_name().to_string_lossy().into_owned();
                for task_file in lfs::list_files_sorted(entry.path())? {
                    if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&task_file) {
                        if spec.task_id == task_id || spec.task_id.starts_with(task_id) {
                            return Ok((node, "RUNNING".to_string()));
                        }
                    }
                }
            }
        }
    }

    // Check done
    let done_dir = root.join("done");
    if done_dir.exists() {
        for entry in std::fs::read_dir(&done_dir)? {
            let entry = entry?;
            if entry.path().is_dir() {
                let node = entry.file_name().to_string_lossy().into_owned();
                for result_file in lfs::list_files_sorted(entry.path())? {
                    if let Ok(result) = lfs::read_json::<models::TaskResult, _>(&result_file) {
                        if result.task_id == task_id || result.task_id.starts_with(task_id) {
                            let state = if result.exit_code == 0 { "DONE" } else { "FAILED" };
                            return Ok((node, state.to_string()));
                        }
                    }
                }
            }
        }
    }

    Err(anyhow::anyhow!("Task {} not found", task_id))
}

fn cancel_pending_task(root: &Path, task_id: &str, node: &str) -> Result<()> {
    let inbox_dir = root.join("inbox").join(node);
    let done_dir = root.join("done").join(node);

    lfs::ensure_dir(&done_dir)?;

    // Find and move the task file
    for task_file in lfs::list_files_sorted(&inbox_dir)? {
        if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&task_file) {
            if spec.task_id == task_id || spec.task_id.starts_with(task_id) {
                // Write a cancelled result
                let result = models::TaskResult {
                    task_id: spec.task_id.clone(),
                    idempotency_key: spec.idempotency_key.clone(),
                    node: node.to_string(),
                    started_at: time::OffsetDateTime::now_utc(),
                    finished_at: time::OffsetDateTime::now_utc(),
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: String::new(),
                    runtime_s: 0.0,
                    command: spec.command.clone(),
                    cwd: spec.cwd.clone(),
                    gpus_requested: spec.gpus,
                    gpus_assigned: String::new(),
                };

                let original_name = task_file.file_name().unwrap().to_string_lossy();
                let result_name = format!("{}.cancelled.json", original_name.trim_end_matches(".json"));
                lfs::atomic_write_json(done_dir.join(&result_name), &result)?;

                // Remove from inbox
                std::fs::remove_file(&task_file)?;
                return Ok(());
            }
        }
    }

    Err(anyhow::anyhow!("Task file not found in inbox"))
}

fn cancel_running_task(root: &Path, task_id: &str, node: &str) -> Result<()> {
    let control_dir = root.join("control").join(node);
    lfs::ensure_dir(&control_dir)?;

    // Write cancel command file
    let cancel_cmd = CancelCommand {
        task_id: task_id.to_string(),
        requested_at: time::OffsetDateTime::now_utc(),
    };

    let filename = format!("cancel_{}_{}.json", task_id, Uuid::new_v4());
    lfs::atomic_write_json(control_dir.join(&filename), &cancel_cmd)?;

    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CancelCommand {
    task_id: String,
    #[serde(with = "time::serde::timestamp")]
    requested_at: time::OffsetDateTime,
}
