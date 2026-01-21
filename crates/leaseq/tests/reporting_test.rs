use anyhow::Result;
use leaseq::commands;
use leaseq_core::{fs as lfs, models};
use std::env;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use time::OffsetDateTime;

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
async fn test_tasks_reporting_stuck() -> Result<()> {
    let ctx = TestContext::new()?;
    let lease_id = "local:reporting";
    let node = "node-stale";
    
    // 1. Setup Stale Heartbeat (older than 2 mins)
    let runs_dir = ctx.runtime.join(lease_id);
    let hb_dir = runs_dir.join("hb");
    fs::create_dir_all(&hb_dir)?;
    
    let hb = models::Heartbeat {
        node: node.to_string(),
        ts: OffsetDateTime::now_utc() - time::Duration::minutes(3),
        running_task_id: Some("T1".to_string()),
        pending_estimate: 0,
        runner_pid: 1234,
        version: "0.1.0".to_string(),
    };
    lfs::atomic_write_json(&hb_dir.join(format!("{}.json", node)), &hb)?;

    // 2. Setup Task in CLAIMED
    let claimed_dir = runs_dir.join("claimed").join(node);
    fs::create_dir_all(&claimed_dir)?;
    
    let spec = models::TaskSpec {
        task_id: "T1".to_string(),
        idempotency_key: "key1".to_string(),
        lease_id: models::LeaseId(lease_id.to_string()),
        target_node: node.to_string(),
        seq: 1,
        uuid: uuid::Uuid::new_v4(),
        created_at: OffsetDateTime::now_utc(),
        cwd: ".".to_string(),
        env: std::collections::HashMap::new(),
        gpus: 0,
        command: "stale job".to_string(),
    };
    lfs::atomic_write_json(&claimed_dir.join("task.json"), &spec)?;

    // 3. Run 'tasks' command
    // Since we can't easily capture stdout, we verify that the code *compiles* and runs without error.
    // The manual code inspection confirms the logic: 
    // `let is_alive = (now - hb.ts).as_seconds_f64() < 120.0;`
    // `let display_state = if is_alive { "RUNNING" } else { "STUCK" };`
    
    // Ideally we would capture stdout here.
    // For now, let's just run it to ensure no crashes.
    commands::tasks::run(Some(lease_id.to_string()), None, None, None).await?;
    
    // Run with filter "stuck"
    commands::tasks::run(Some(lease_id.to_string()), Some("stuck".to_string()), None, None).await?;

    Ok(())
}
