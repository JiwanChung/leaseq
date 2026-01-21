use anyhow::Result;
use leaseq_core::{config, fs as lfs, models};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
pub enum TaskStateFilter {
    All,
    Pending,
    Running,
    Done,
    Failed,
    Stuck,
}

impl TaskStateFilter {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "all" => Some(Self::All),
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "done" | "finished" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "stuck" | "unresponsive" => Some(Self::Stuck),
            _ => None,
        }
    }
}

pub async fn run(
    lease: Option<String>,
    state: Option<String>,
    node: Option<String>,
    search: Option<String>,
) -> Result<()> {
    let lease_id = lease.unwrap_or_else(config::local_lease_id);

    let root = if lease_id.starts_with("local:") {
        config::runtime_dir().join(&lease_id)
    } else {
        config::leaseq_home_dir().join("runs").join(&lease_id)
    };

    let state_filter = state
        .as_ref()
        .and_then(|s| TaskStateFilter::from_str(s))
        .unwrap_or(TaskStateFilter::All);

    // Load heartbeats to check node liveness
    let mut node_status = HashMap::new();
    let hb_dir = root.join("hb");
    let now = time::OffsetDateTime::now_utc();
    if hb_dir.exists() {
        for f in lfs::list_files_sorted(&hb_dir).unwrap_or_default() {
            if let Ok(hb) = lfs::read_json::<models::Heartbeat, _>(&f) {
                // If HB is older than 2 mins (same threshold as add.rs), consider stale
                let is_alive = (now - hb.ts).as_seconds_f64() < 120.0;
                node_status.insert(hb.node, is_alive);
            }
        }
    }

    println!("Lease: {}", lease_id);
    println!("{:<10} {:<10} {:<12} COMMAND", "TASK", "STATE", "NODE");
    println!("{}", "-".repeat(60));

    // Collect and display tasks
    let mut task_count = 0;

    // Running (or Stuck) tasks (claimed)
    if state_filter == TaskStateFilter::All 
        || state_filter == TaskStateFilter::Running 
        || state_filter == TaskStateFilter::Stuck 
    {
        let claimed_dir = root.join("claimed");
        if claimed_dir.exists() {
            for entry in std::fs::read_dir(&claimed_dir)? {
                let entry = entry?;
                if entry.path().is_dir() {
                    let node_name = entry.file_name().to_string_lossy().into_owned();

                    if let Some(ref n) = node {
                        if &node_name != n {
                            continue;
                        }
                    }

                    // Check liveness
                    // If no heartbeat found, assume dead/stuck (safe default)
                    let is_alive = *node_status.get(&node_name).unwrap_or(&false);
                    let display_state = if is_alive { "RUNNING" } else { "STUCK" };

                    // Apply filter
                    if state_filter == TaskStateFilter::Running && !is_alive {
                        continue;
                    }
                    if state_filter == TaskStateFilter::Stuck && is_alive {
                        continue;
                    }

                    for task_file in lfs::list_files_sorted(entry.path())? {
                        if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&task_file) {
                            if let Some(ref s) = search {
                                if !spec.command.contains(s) && !spec.task_id.contains(s) {
                                    continue;
                                }
                            }
                            println!(
                                "{:<10} {:<10} {:<12} {}",
                                spec.task_id,
                                display_state,
                                node_name,
                                truncate(&spec.command, 40)
                            );
                            task_count += 1;
                        }
                    }
                }
            }
        }
    }

    // Pending tasks (inbox)
    if state_filter == TaskStateFilter::All || state_filter == TaskStateFilter::Pending {
        let inbox_dir = root.join("inbox");
        if inbox_dir.exists() {
            for entry in std::fs::read_dir(&inbox_dir)? {
                let entry = entry?;
                if entry.path().is_dir() {
                    let node_name = entry.file_name().to_string_lossy().into_owned();

                    if let Some(ref n) = node {
                        if &node_name != n {
                            continue;
                        }
                    }

                    for task_file in lfs::list_files_sorted(entry.path())? {
                        if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&task_file) {
                            if let Some(ref s) = search {
                                if !spec.command.contains(s) && !spec.task_id.contains(s) {
                                    continue;
                                }
                            }
                            println!(
                                "{:<10} {:<10} {:<12} {}",
                                spec.task_id,
                                "PENDING",
                                node_name,
                                truncate(&spec.command, 40)
                            );
                            task_count += 1;
                        }
                    }
                }
            }
        }
    }

    // Done/Failed tasks
    if state_filter == TaskStateFilter::All
        || state_filter == TaskStateFilter::Done
        || state_filter == TaskStateFilter::Failed
    {
        let done_dir = root.join("done");
        if done_dir.exists() {
            for entry in std::fs::read_dir(&done_dir)? {
                let entry = entry?;
                if entry.path().is_dir() {
                    let node_name = entry.file_name().to_string_lossy().into_owned();

                    if let Some(ref n) = node {
                        if &node_name != n {
                            continue;
                        }
                    }

                    for result_file in lfs::list_files_sorted(entry.path())? {
                        // Only process result files
                        if !result_file
                            .file_name()
                            .map(|n| n.to_string_lossy().ends_with(".result.json"))
                            .unwrap_or(false)
                        {
                            continue;
                        }

                        if let Ok(result) = lfs::read_json::<models::TaskResult, _>(&result_file) {
                            let task_state = if result.exit_code == 0 { "DONE" } else { "FAILED" };

                            // Filter by state
                            if state_filter == TaskStateFilter::Done && result.exit_code != 0 {
                                continue;
                            }
                            if state_filter == TaskStateFilter::Failed && result.exit_code == 0 {
                                continue;
                            }

                            if let Some(ref s) = search {
                                if !result.task_id.contains(s) && !result.command.contains(s) {
                                    continue;
                                }
                            }

                            let cmd_display = if result.command.is_empty() {
                                format!("exit={}", result.exit_code)
                            } else {
                                truncate(&result.command, 40)
                            };
                            println!(
                                "{:<10} {:<10} {:<12} {}",
                                result.task_id, task_state, result.node, cmd_display
                            );
                            task_count += 1;
                        }
                    }
                }
            }
        }
    }

    println!("{}", "-".repeat(60));
    println!("Total: {} tasks", task_count);

    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
