use anyhow::Result;
use leaseq::commands;
use leaseq_core::{models, fs as lfs};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use time::OffsetDateTime;

struct TestContext {
    _temp_dir: TempDir,
    runtime: PathBuf,
    home: PathBuf,
}

impl TestContext {
    fn new() -> Result<Self> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path().join(".leaseq");
        fs::create_dir_all(&home)?;
        
        let runtime = temp_dir.path().join("runtime");
        fs::create_dir_all(&runtime)?;

        env::set_var("LEASEQ_HOME", &home);
        env::set_var("LEASEQ_RUNTIME_DIR", &runtime);

        Ok(Self {
            _temp_dir: temp_dir,
            runtime,
            home,
        })
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        env::remove_var("LEASEQ_HOME");
        env::remove_var("LEASEQ_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn test_add_picks_dead_node() -> Result<()> {
    // Tests that 'add' checks heartbeat timestamp and REJECTS dead nodes.
    let ctx = TestContext::new()?;
    // Use a non-local lease ID so 'add' checks heartbeats instead of assuming localhost
    let lease_id = "job-dead-node-test"; 
    
    // Setup a "dead" node heartbeat in LEASEQ_HOME/runs/<lease_id>
    let runs_dir = ctx.home.join("runs").join(lease_id);
    let hb_dir = runs_dir.join("hb");
    fs::create_dir_all(&hb_dir)?;
    
    let dead_node = "dead-node";
    let hb_file = hb_dir.join(format!("{}.json", dead_node));
    
    let old_time = OffsetDateTime::now_utc() - time::Duration::hours(1);
    let hb = models::Heartbeat {
        node: dead_node.to_string(),
        ts: old_time,
        running_task_id: None,
        pending_estimate: 0,
        runner_pid: 1234,
        version: "0.1.0".to_string(),
    };
    lfs::atomic_write_json(&hb_file, &hb)?;

    // Add task without specifying node - Should FAIL now
    let result = commands::add::run(vec!["echo".to_string(), "foo".to_string()], Some(lease_id.to_string()), None).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("No active nodes found"));

    Ok(())
}

#[tokio::test]
async fn test_multiple_runners_concurrency() -> Result<()> {
    let ctx = TestContext::new()?;
    let lease_id = "local:concurrent";
    
    // We will manually inject tasks for two nodes to simulate load distribution
    // (since 'add' doesn't round-robin yet)
    let node1 = "node-1";
    let node2 = "node-2";
    
    // Setup dirs
    let runs_dir = ctx.runtime.join(lease_id);
    for node in [node1, node2] {
        let inbox = runs_dir.join("inbox").join(node);
        fs::create_dir_all(&inbox)?;
        
        let spec = models::TaskSpec {
            task_id: format!("T-{}", node),
            idempotency_key: format!("key-{}", node),
            lease_id: models::LeaseId(lease_id.to_string()),
            target_node: node.to_string(),
            seq: 1,
            uuid: uuid::Uuid::new_v4(),
            created_at: OffsetDateTime::now_utc(),
            cwd: ".".to_string(),
            env: std::collections::HashMap::new(),
            gpus: 0,
            command: format!("echo executed on {}", node),
        };
        let f = inbox.join("task.json");
        lfs::atomic_write_json(&f, &spec)?;
    }

    // Spawn two runners concurrently
    let run_node1 = commands::run::run(commands::run::RunArgs {
        lease: lease_id.to_string(),
        node: Some(node1.to_string()),
        root: None,
    });
    
    let run_node2 = commands::run::run(commands::run::RunArgs {
        lease: lease_id.to_string(),
        node: Some(node2.to_string()),
        root: None,
    });

    // Let them run for a bit (they loop forever, so we need to timeout)
    // We use tokio::join! to start both, but wrap in timeout
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(run_node1, run_node2)
    }).await;

    // Verify both executed
    for node in [node1, node2] {
        let done_dir = runs_dir.join("done").join(node);
        let mut found = false;
        if done_dir.exists() {
            for entry in fs::read_dir(&done_dir)? {
                let entry = entry?;
                let content = fs::read_to_string(entry.path())?;
                if content.contains(&format!("executed on {}", node)) {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "Node {} did not execute its task", node);
    }

    Ok(())
}

#[tokio::test]
async fn test_blocking_task_heartbeat_gap() -> Result<()> {
    let ctx = TestContext::new()?;
    let lease_id = "local:blocking";
    let node = "node-block";
    
    // 1. Submit a sleeping task (Long enough to cover heartbeat interval of 5s)
    commands::add::run(
        vec!["sleep".to_string(), "7".to_string()], 
        Some(lease_id.to_string()), 
        Some(node.to_string())
    ).await?;

    // 2. Start runner in background task
    let run_fut = commands::run::run(commands::run::RunArgs {
        lease: lease_id.to_string(),
        node: Some(node.to_string()),
        root: None,
    });
    
    // We want to sample the heartbeat file WHILE it is running.
    
    let check_heartbeat = async {
        // Wait for runner to start and pick up task (give it 1s)
        tokio::time::sleep(Duration::from_secs(1)).await;
        
        let hb_file = ctx.runtime.join(lease_id).join("hb").join(format!("{}.json", node));
        
        // Read initial heartbeat
        let hb1: models::Heartbeat = lfs::read_json(&hb_file).expect("HB file missing");
        
        // Wait 5.5s (task still sleeping, HB interval is 5s, so it should update)
        tokio::time::sleep(Duration::from_millis(5500)).await;
        
        // Read again
        let hb2: models::Heartbeat = lfs::read_json(&hb_file)?;
        
        // NOW we expect hb2.ts > hb1.ts because background thread should be updating it!
        
        if hb2.ts > hb1.ts {
            // Success!
            Ok::<bool, anyhow::Error>(true)
        } else {
            // Failed: Still blocked
            Ok::<bool, anyhow::Error>(false)
        }
    };

    let (runner_res, check_res) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(8), run_fut),
        check_heartbeat
    );

    // Runner should timeout (we killed it)
    assert!(runner_res.is_err()); // Timeout elapsed
    
    // Check result
    let is_updating = check_res?;
    assert!(is_updating, "Heartbeat did NOT update during blocking task!");

    Ok(())
}
