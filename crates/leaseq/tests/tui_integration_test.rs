use anyhow::Result;
use leaseq::tui::app::{App, TaskFilter};
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

#[test]
fn test_tui_stuck_task_state() -> Result<()> {
    let ctx = TestContext::new()?;
    let lease_id = "local:tui-stuck";
    let node = "node-stale";
    
    let runs_dir = ctx.runtime.join(lease_id);
    let hb_dir = runs_dir.join("hb");
    let claimed_dir = runs_dir.join("claimed").join(node);
    let inbox_dir = runs_dir.join("inbox").join(node);
    fs::create_dir_all(&hb_dir)?;
    fs::create_dir_all(&claimed_dir)?;
    fs::create_dir_all(&inbox_dir)?;

    // 1. Setup Stale Heartbeat
    let hb = models::Heartbeat {
        node: node.to_string(),
        ts: OffsetDateTime::now_utc() - time::Duration::minutes(5),
        running_task_id: Some("T1".to_string()),
        pending_estimate: 0,
        runner_pid: 1234,
        version: "0.1.0".to_string(),
    };
    lfs::atomic_write_json(&hb_dir.join(format!("{}.json", node)), &hb)?;

    // 2. Setup Task in CLAIMED
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

    // Verify file is readable
    let task_path = claimed_dir.join("task.json");
    assert!(task_path.exists());
    let read_spec: models::TaskSpec = lfs::read_json(&task_path).expect("Failed to read task json");
    assert_eq!(read_spec.task_id, "T1");

    // 3. Initialize App and Refresh
    let mut app = App::new(Some(lease_id.to_string()));
    app.refresh_data();

    println!("All tasks: {:?}", app.all_tasks);
    println!("Filtered tasks: {:?}", app.tasks);

    // 4. Verify Task State
    assert!(!app.all_tasks.is_empty(), "all_tasks should not be empty");
    assert!(!app.tasks.is_empty(), "tasks (filtered) should not be empty");
    let task = &app.tasks[0];
    assert_eq!(task.id, "T1");
    assert_eq!(task.state, "STUCK"); // Must be STUCK, not RUNNING

    // 5. Test Filters
    // Set filter to Running -> Should be empty
    app.filter_state.filter = TaskFilter::Running;
    app.apply_filter();
    assert!(app.tasks.is_empty());

    // Set filter to Stuck -> Should show T1
    app.filter_state.filter = TaskFilter::Stuck;
    app.apply_filter();
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app.tasks[0].id, "T1");

    Ok(())
}

#[test]
fn test_tui_recovery_action() -> Result<()> {
    // This tests the logic that would happen if the user selected "Recover" in the UI.
    // Since we can't simulate keypresses easily without a TUI backend, we manually check the logic
    // that the `handle_task_actions_input` method performs (atomic rename).
    
    let ctx = TestContext::new()?;
    let lease_id = "local:tui-recover";
    let node = "node-recover";
    
    let runs_dir = ctx.runtime.join(lease_id);
    let claimed_dir = runs_dir.join("claimed").join(node);
    let inbox_dir = runs_dir.join("inbox").join(node);
    fs::create_dir_all(&claimed_dir)?;
    fs::create_dir_all(&inbox_dir)?;

    // Setup Task in CLAIMED
    let task_id = "T-REC";
    let spec = models::TaskSpec {
        task_id: task_id.to_string(),
        idempotency_key: "rec".to_string(),
        lease_id: models::LeaseId(lease_id.to_string()),
        target_node: node.to_string(),
        seq: 1,
        uuid: uuid::Uuid::new_v4(),
        created_at: OffsetDateTime::now_utc(),
        cwd: ".".to_string(),
        env: std::collections::HashMap::new(),
        gpus: 0,
        command: "recover me".to_string(),
    };
    lfs::atomic_write_json(&claimed_dir.join("task.json"), &spec)?;

    // Perform Recovery (Logic from App::handle_task_actions_input)
    // We simulate finding the file and moving it.
    
    let mut found = false;
    if let Ok(files) = lfs::list_files_sorted(&claimed_dir) {
         for f in files {
             if let Ok(s) = lfs::read_json::<models::TaskSpec, _>(&f) {
                 if s.task_id == task_id {
                     let new_path = inbox_dir.join(f.file_name().unwrap());
                     std::fs::rename(&f, &new_path)?;
                     found = true;
                     break;
                 }
             }
         }
    }
    assert!(found, "Failed to find task to recover");

    // Verify it's in inbox
    let inbox_files = lfs::list_files_sorted(&inbox_dir)?;
    assert_eq!(inbox_files.len(), 1);
    assert!(inbox_files[0].to_string_lossy().contains("task.json"));
    
    // Verify claimed is empty
    let claimed_files = lfs::list_files_sorted(&claimed_dir)?;
    assert!(claimed_files.is_empty());

    Ok(())
}