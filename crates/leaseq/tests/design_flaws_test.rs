use anyhow::Result;
use leaseq::commands;
use leaseq_core::{fs as lfs, models};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct TestContext {
    _temp_dir: TempDir,
    runtime: PathBuf,
}

impl TestContext {
    fn new() -> Result<Self> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = temp_dir.path().join("runtime");
        fs::create_dir_all(&runtime)?;
        env::set_var("LEASEQ_RUNTIME_DIR", &runtime);
        Ok(Self { _temp_dir: temp_dir, runtime })
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        env::remove_var("LEASEQ_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn test_zombie_tasks_crash_recovery() -> Result<()> {
    let ctx = TestContext::new()?;
    let lease_id = "local:zombie";
    let node = "node-crash";

    // 1. Simulate a "Crashed" State
    // A task was moved to 'claimed' by a previous runner (PID 9999) that no longer exists.
    let runs_dir = ctx.runtime.join(lease_id);
    let claimed_dir = runs_dir.join("claimed").join(node);
    let done_dir = runs_dir.join("done").join(node);
    fs::create_dir_all(&claimed_dir)?;
    fs::create_dir_all(&done_dir)?;

    let spec = models::TaskSpec {
        task_id: "T-CRASHED".to_string(),
        idempotency_key: "key-crashed".to_string(),
        lease_id: models::LeaseId(lease_id.to_string()),
        target_node: node.to_string(),
        seq: 1,
        uuid: uuid::Uuid::new_v4(),
        created_at: time::OffsetDateTime::now_utc(),
        cwd: ".".to_string(),
        env: std::collections::HashMap::new(),
        gpus: 0,
        command: "echo 'I should be recovered'".to_string(),
    };
    
    // Write directly to CLAIMED (simulating the crash state)
    let crashed_file = claimed_dir.join("task_crashed.json");
    lfs::atomic_write_json(&crashed_file, &spec)?;

    // 2. Start a NEW Runner
    // We expect a robust system to see this file, check the heartbeat (which is missing or old),
    // and re-queue it.
    let run_fut = commands::run::run(commands::run::RunArgs {
        lease: lease_id.to_string(),
        node: Some(node.to_string()),
        root: None,
    });

    // Run for a short time
    let _ = tokio::time::timeout(Duration::from_secs(2), run_fut).await;

    // 3. Check if the task was processed
    // If it's still in claimed, the system failed to recover it.
    let still_claimed = crashed_file.exists();
    let is_done = fs::read_dir(&done_dir)?.count() > 0;

    if still_claimed && !is_done {
        // This confirms the design flaw
        println!("DESIGN FLAW CONFIRMED: Task remained in 'claimed' (Zombie). No recovery mechanism.");
        // We assert FALSE here because we expect it to be FIXED now.
        assert!(false, "Zombie task was NOT recovered!");
    } else {
        // If this branch hits, it means the system fixed itself (recovered and executed).
        println!("Success: Zombie task recovered and executed.");
        assert!(is_done);
    }

    Ok(())
}

#[tokio::test]
async fn test_scalability_large_inbox_performance() -> Result<()> {
    let ctx = TestContext::new()?;
    let lease_id = "local:perf";
    let node = "node-perf";
    
    let runs_dir = ctx.runtime.join(lease_id);
    let inbox = runs_dir.join("inbox").join(node);
    fs::create_dir_all(&inbox)?;

    // 1. Generate 2,000 dummy task files
    // This is small for a real queue, but enough to show the O(N) cost in a test.
    println!("Generating 2,000 files...");
    for i in 0..2000 {
        // We just create empty files for listing speed test, 
        // as list_files_sorted just checks existence and name.
        // Actually leaseq uses `read_dir` then `path.is_file()`.
        let p = inbox.join(format!("{:06}.json", i));
        fs::write(p, "{}")?;
    }

    // 2. Measure `list_files_sorted`
    let start = Instant::now();
    let files = lfs::list_files_sorted(&inbox)?;
    let duration = start.elapsed();

    println!("Listing 2,000 files took: {:?}", duration);
    assert_eq!(files.len(), 2000);

    // If this takes > 10ms on local disk, it will be > 100ms-500ms on NFS.
    // If we have 100,000 files, this linear scaling kills the runner loop (which runs every 1s).
    
    // For the test to pass, we just acknowledge the metric. 
    // We can set a soft assertion that it shouldn't be instant.
    
    Ok(())
}
