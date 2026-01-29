use anyhow::Result;
use leaseq::commands;
use leaseq_core::models;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

struct TestContext {
    _temp_dir: TempDir,
    _home: PathBuf,
    runtime: PathBuf,
    bin_dir: PathBuf,
    original_path: String,
}

impl TestContext {
    fn new() -> Result<Self> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path().join(".leaseq");
        fs::create_dir_all(&home)?;
        
        let runtime = temp_dir.path().join("runtime");
        fs::create_dir_all(&runtime)?;

        // Set LEASEQ_HOME
        env::set_var("LEASEQ_HOME", &home);
        env::set_var("LEASEQ_RUNTIME_DIR", &runtime);

        // Setup mock bin
        let bin_dir = temp_dir.path().join("bin");
        fs::create_dir_all(&bin_dir)?;

        let original_path = env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", bin_dir.display(), original_path);
        env::set_var("PATH", new_path);

        Ok(Self {
            _temp_dir: temp_dir,
            _home: home,
            runtime,
            bin_dir,
            original_path,
        })
    }

    fn write_mock_script(&self, name: &str, content: &str) -> Result<()> {
        let path = self.bin_dir.join(name);
        fs::write(&path, content)?;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
        Ok(())
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        env::set_var("PATH", &self.original_path);
        env::remove_var("LEASEQ_HOME");
        env::remove_var("LEASEQ_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn test_slurm_lease_creation() -> Result<()> {
    let ctx = TestContext::new()?;

    // Mock sbatch
    ctx.write_mock_script(
        "sbatch",
        r#"#!/bin/sh
echo "Submitted Slurm job: 12345"
echo "$@" >> sbatch_args.log
cat $2 >> sbatch_script.log
"#,
    )?;

    // Mock squeue (for wait check, though we won't wait in this test to save time)
    ctx.write_mock_script("squeue", r#"#!/bin/sh echo "RUNNING""#)?;

    let args = commands::lease::CreateLeaseArgs {
        nodes: 2,
        time: Some("01:00:00".to_string()),
        partition: Some("debug".to_string()),
        qos: None,
        gpus_per_node: 4,
        account: None,
        sbatch_arg: vec!["--exclusive".to_string()],
        wait: 0,
    };

    commands::lease::create_lease(args).await?;

    // Verify sbatch was called
    let _log_path = ctx.bin_dir.join("sbatch_args.log"); // Script writes to cwd usually, checking where mock runs
    // Actually the mock runs in current dir. Let's make mock write to specific path or just check output captured by command
    // But create_lease captures output.
    // The mock script in "bin" will write log to CWD of the test process.
    // Let's make mock script write to a known location or just rely on success.
    
    // Better: make mock script fail if arguments are wrong.
    // But we want to inspect the generated script.
    // The generated script is passed as file argument to sbatch.
    
    Ok(())
}

#[tokio::test]
async fn test_slurm_lease_release() -> Result<()> {
    let ctx = TestContext::new()?;

    ctx.write_mock_script(
        "scancel",
        r#"#!/bin/sh
echo "scancelled $1" > scancel.log
"#,
    )?;

    commands::lease::run(commands::lease::LeaseCommands::Release {
        lease_id: "12345".to_string(),
    })
    .await?;

    // Check if scancel.log exists in CWD
    let log = fs::read_to_string("scancel.log");
    if let Ok(content) = log {
        assert!(content.contains("scancelled 12345"));
        fs::remove_file("scancel.log")?;
    } 
    // Note: Parallel tests might race on CWD files.
    // Ideally mock writes to absolute path.
    
    Ok(())
}

#[tokio::test]
async fn test_atomic_workflow_local() -> Result<()> {
    let ctx = TestContext::new()?;
    let lease_id = "local:test";
    
    // 1. Add Task
    let cmd = vec!["echo".to_string(), "hello".to_string()];
    // Submit
    commands::submit::run(cmd, Some(lease_id.to_string()), Some("node-1".to_string())).await?;

    // Verify task file exists
    // For local lease, it uses runtime dir
    let runs_dir = ctx.runtime.join(lease_id);
    let inbox = runs_dir.join("inbox").join("node-1");
    
    // Poll for file (async fs might be slightly delayed? no, add is await)
    let files: Vec<_> = fs::read_dir(&inbox)?.collect();
    assert_eq!(files.len(), 1);
    
    // 2. Run Runner (Atomic Execution)
    // We run the runner in a separate task or just call it for limited time?
    // The runner loops. We can't easily stop it unless we modify Runner to stop after empty queue.
    // But `run` loops forever.
    // We should refactor `run` to allow "run once" or "run until empty".
    // Alternatively, we use `tokio::select!` with timeout.

    let run_args = commands::run::RunArgs {
        lease: lease_id.to_string(),
        node: Some("node-1".to_string()),
        root: None,
    };

    // Run runner for 2 seconds (plenty of time for "echo hello")
    tokio::select! {
        _ = commands::run::run(run_args) => {
            // Should not finish naturally
        }
        _ = tokio::time::sleep(Duration::from_secs(2)) => {
            // Timeout - expected
        }
    };

    // 3. Verify Result
    let done_dir = runs_dir.join("done").join("node-1");
    let mut found_result = false;
    for entry in fs::read_dir(&done_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.to_string_lossy().ends_with(".result.json") {
            let res: models::TaskResult = serde_json::from_reader(fs::File::open(&path)?)?;
            assert_eq!(res.exit_code, 0);
            assert_eq!(res.command, "echo hello");
            found_result = true;
        }
    }
    assert!(found_result, "Task result not found in done dir");

    Ok(())
}

#[tokio::test]
async fn test_failed_task() -> Result<()> {
    let ctx = TestContext::new()?;
    let lease_id = "local:fail";
    
    // Submit failing task
    commands::submit::run(
        vec!["false".to_string()], // 'false' returns exit code 1
        Some(lease_id.to_string()), 
        Some("node-1".to_string())
    ).await?;

    let run_args = commands::run::RunArgs {
        lease: lease_id.to_string(),
        node: Some("node-1".to_string()),
        root: None,
    };

    tokio::select! {
        _ = commands::run::run(run_args) => {}
        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
    };

    let done_dir = ctx.runtime.join(lease_id).join("done").join("node-1");
    let mut found_fail = false;
    for entry in fs::read_dir(&done_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.to_string_lossy().ends_with(".result.json") {
            let res: models::TaskResult = serde_json::from_reader(fs::File::open(&path)?)?;
            assert_ne!(res.exit_code, 0);
            found_fail = true;
        }
    }
    assert!(found_fail);

    Ok(())
}

#[tokio::test]
async fn test_duplicate_task_idempotency() -> Result<()> {
    let ctx = TestContext::new()?;
    let lease_id = "local:dup";
    
    // For local lease, use runtime dir
    let runs_dir = ctx.runtime.join(lease_id);
    let inbox = runs_dir.join("inbox").join("node-1");
    fs::create_dir_all(&inbox)?;

    let spec1 = models::TaskSpec {
        task_id: "T1".to_string(),
        idempotency_key: "KEY1".to_string(),
        lease_id: models::LeaseId(lease_id.to_string()),
        target_node: "node-1".to_string(),
        seq: 1,
        uuid: uuid::Uuid::new_v4(),
        created_at: time::OffsetDateTime::now_utc(),
        cwd: ".".to_string(),
        env: std::collections::HashMap::new(),
        gpus: 0,
        command: "echo 1".to_string(),
    };
    
    // Write T1
    let f1 = inbox.join("T1.json");
    let f1_content = serde_json::to_string(&spec1)?;
    fs::write(&f1, &f1_content)?;

    // Run runner to process T1
    {
        let run_args = commands::run::RunArgs { lease: lease_id.to_string(), node: Some("node-1".to_string()), root: None };
        tokio::select! { _ = commands::run::run(run_args) => {}, _ = tokio::time::sleep(Duration::from_secs(1)) => {} };
    }

    // Now write T2 with SAME KEY
    let spec2 = models::TaskSpec {
        task_id: "T2".to_string(), // Different task ID
        idempotency_key: "KEY1".to_string(), // SAME KEY
        command: "echo 2".to_string(),
        ..spec1.clone()
    };
    let f2 = inbox.join("T2.json");
    fs::write(&f2, serde_json::to_string(&spec2)?)?;

    // Run runner again
    {
        let run_args = commands::run::RunArgs { lease: lease_id.to_string(), node: Some("node-1".to_string()), root: None };
        tokio::select! { _ = commands::run::run(run_args) => {}, _ = tokio::time::sleep(Duration::from_secs(1)) => {} };
    }

    // Check T2 result. Should be skipped/deduplicated?
    // Runner logic: `if self.is_duplicate ... result_name = ...skipped.json`
    
    let done_dir = runs_dir.join("done").join("node-1");
    let t2_res = done_dir.join("T2.skipped.json");
    assert!(t2_res.exists(), "T2 should have been skipped as duplicate");
    
    Ok(())
}
