use clap::Parser;
use leaseq_core::{fs as lfs, models, config};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn, error};
use anyhow::{Result, Context};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
   /// Lease ID (e.g., local:myhost or slurm jobid)
   #[arg(long)]
   lease: String,

   /// Node name (defaults to hostname)
   #[arg(long)]
   node: Option<String>,

   /// Root directory for execution (overrides default lookup)
   #[arg(long)]
   root: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let hostname = hostname::get()?.to_string_lossy().into_owned();
    let node = args.node.unwrap_or_else(|| hostname.clone());
    
    let root = if let Some(r) = args.root {
        r
    } else {
        // Inference logic
        if args.lease.starts_with("local:") {
             config::runtime_dir().join(&args.lease)
        } else {
             config::leaseq_home_dir().join("runs").join(&args.lease)
        }
    };

    info!("Starting runner for lease={} node={} root={:?}", args.lease, node, root);

    // Ensure directory structure exists
    let dirs = ["inbox", "claimed", "ack", "done", "logs", "hb", "events"];
    for d in &dirs {
        let p = root.join(d).join(&node);
        lfs::ensure_dir(&p).context(format!("Failed to create {}", p.display()))?;
    }
    // Also base logs dir (might be shared?)
    // The design says `logs/T014.out`. It doesn't explicitly say `logs/<node>/`.
    // "Each task owns: logs/T014.out". 
    // "Path: inbox/<node>/..."
    // "Path: done/<node>/..."
    // "Path: logs/" (shared usually? or per node?)
    // Design doc: "logs/T014.out" suggests flat logs dir for the lease?
    // "Archive storage ... ~/.leaseq/runs/local:<host>/logs/"
    // So yes, `logs/` seems to be at the lease root level, not per node.
    lfs::ensure_dir(root.join("logs"))?;

    let mut runner = Runner {
        _lease_id: args.lease,
        node,
        root,
        executed_keys: std::collections::HashSet::new(),
    };

    // Load existing executed keys from done directory
    runner.load_executed_keys()?;

    runner.run().await
}

struct Runner {
    _lease_id: String,
    node: String,
    root: PathBuf,
    /// Cache of executed idempotency keys for dedupe
    executed_keys: std::collections::HashSet<String>,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)]
struct CancelCommand {
    task_id: String,
}

impl Runner {
    /// Load executed idempotency keys from done directory at startup
    fn load_executed_keys(&mut self) -> Result<()> {
        let done_dir = self.root.join("done").join(&self.node);
        if !done_dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(&done_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && path.file_name().map(|n| n.to_string_lossy().ends_with(".result.json")).unwrap_or(false)
            {
                if let Ok(result) = lfs::read_json::<models::TaskResult, _>(&path) {
                    self.executed_keys.insert(result.idempotency_key);
                }
            }
        }

        info!("Loaded {} executed keys from done directory", self.executed_keys.len());
        Ok(())
    }

    /// Check if a task has already been executed (dedupe)
    fn is_duplicate(&self, idempotency_key: &str) -> bool {
        self.executed_keys.contains(idempotency_key)
    }

    /// Check for cancel commands for a specific task
    #[allow(dead_code)]
    fn check_cancel(&self, task_id: &str) -> bool {
        let control_dir = self.root.join("control").join(&self.node);
        if !control_dir.exists() {
            return false;
        }

        if let Ok(entries) = std::fs::read_dir(&control_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.file_name()
                    .map(|n| n.to_string_lossy().starts_with("cancel_"))
                    .unwrap_or(false)
                {
                    if let Ok(cmd) = lfs::read_json::<CancelCommand, _>(&path) {
                        if cmd.task_id == task_id || task_id.starts_with(&cmd.task_id) {
                            // Remove the cancel file after processing
                            let _ = std::fs::remove_file(&path);
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    async fn run(&mut self) -> Result<()> {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;

            if let Err(e) = self.update_heartbeat(None).await {
                error!("Heartbeat failed: {}", e);
            }

            match self.poll_and_claim().await {
                Ok(Some(task_path)) => {
                    if let Err(e) = self.execute_task(&task_path).await {
                        error!("Task execution failed: {}", e);
                    }
                }
                Ok(None) => {
                    // Idle
                }
                Err(e) => {
                    error!("Poll failed: {}", e);
                }
            }
        }
    }

    async fn update_heartbeat(&self, running_task: Option<&str>) -> Result<()> {
        let hb_path = self.root.join("hb").join(format!("{}.json", self.node));
        // Ensure hb dir exists (it should, but just in case)
        lfs::ensure_dir(hb_path.parent().unwrap())?;

        let hb = models::Heartbeat {
            node: self.node.clone(),
            ts: time::OffsetDateTime::now_utc(),
            running_task_id: running_task.map(|s| s.to_string()),
            pending_estimate: 0, // TODO: count inbox
            runner_pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        
        lfs::atomic_write_json(&hb_path, &hb)?;
        Ok(())
    }

    async fn poll_and_claim(&self) -> Result<Option<PathBuf>> {
        let inbox_dir = self.root.join("inbox").join(&self.node);
        let entries = lfs::list_files_sorted(&inbox_dir)?;
        
        if let Some(task_file) = entries.first() {
            let filename = task_file.file_name().unwrap();
            let claimed_dir = self.root.join("claimed").join(&self.node);
            let claimed_path = claimed_dir.join(filename);
            
            info!("Claiming task: {:?}", filename);
            
            // Atomic rename to claim
            match std::fs::rename(task_file, &claimed_path) {
                Ok(_) => {
                    // Send Ack
                     // "write ack/<node>/T014.ack.json" - wait, task filename has ID?
                     // filename: 000012_T014_<uuid>.json
                     // We need to parse it or read it to get task ID for Ack filename.
                     // But reading after rename is safe.
                     return Ok(Some(claimed_path));
                },
                Err(e) => {
                    warn!("Failed to claim (race condition?): {}", e);
                    return Ok(None);
                }
            }
        }
        
        Ok(None)
    }

    async fn execute_task(&mut self, task_path: &Path) -> Result<()> {
        // Read task spec
        let spec: models::TaskSpec = lfs::read_json(task_path)?;
        info!("Executing task {} ({})", spec.task_id, spec.command);

        let done_dir = self.root.join("done").join(&self.node);

        // Dedupe check: check if this idempotency key has already been executed
        if self.is_duplicate(&spec.idempotency_key) {
            warn!("Skipping duplicate task {} (key={})", spec.task_id, spec.idempotency_key);

            // Write SKIPPED result
            let result = models::TaskResult {
                task_id: spec.task_id.clone(),
                idempotency_key: spec.idempotency_key.clone(),
                node: self.node.clone(),
                started_at: time::OffsetDateTime::now_utc(),
                finished_at: time::OffsetDateTime::now_utc(),
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
                runtime_s: 0.0,
                command: spec.command.clone(),
                gpus_requested: spec.gpus,
                gpus_assigned: String::new(),
            };

            let original_name = task_path.file_name().unwrap().to_string_lossy();
            let result_name = format!("{}.skipped.json", original_name.trim_end_matches(".json"));
            lfs::atomic_write_json(done_dir.join(&result_name), &result)?;

            // Move task file to done
            let archived_task_path = done_dir.join(task_path.file_name().unwrap());
            std::fs::rename(task_path, &archived_task_path)?;

            return Ok(());
        }

        // Update heartbeat with running task
        self.update_heartbeat(Some(&spec.task_id)).await?;

        // Prepare logs
        let stdout_path = self.root.join("logs").join(format!("{}.out", spec.task_id));
        let stderr_path = self.root.join("logs").join(format!("{}.err", spec.task_id));
        
        let stdout_file = std::fs::File::create(&stdout_path)?;
        let stderr_file = std::fs::File::create(&stderr_path)?;

        let start_time = time::OffsetDateTime::now_utc();
        
        // Spawn process
        // Command string splitting is naive here. In real world "python train.py" needs splitting or shell.
        // Design doc says: `bash -lc '...'`
        let status = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(&spec.command)
            .current_dir(if Path::new(&spec.cwd).exists() { &spec.cwd } else { "." })
            .stdout(stdout_file)
            .stderr(stderr_file)
            .envs(&spec.env)
            .status()
            .await?;

        let end_time = time::OffsetDateTime::now_utc();
        let runtime = (end_time - start_time).as_seconds_f64();

        info!("Task {} finished with {}", spec.task_id, status);

        // Write Result
        // TODO: Implement proper GPU assignment and track in gpus_assigned
        let gpus_assigned = if spec.gpus > 0 {
            // For now, assign GPUs 0..N-1 (placeholder logic)
            (0..spec.gpus).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        } else {
            String::new()
        };

        let result = models::TaskResult {
            task_id: spec.task_id.clone(),
            idempotency_key: spec.idempotency_key.clone(),
            node: self.node.clone(),
            started_at: start_time,
            finished_at: end_time,
            exit_code: status.code().unwrap_or(-1),
            stdout: format!("logs/{}.out", spec.task_id),
            stderr: format!("logs/{}.err", spec.task_id),
            runtime_s: runtime,
            command: spec.command.clone(),
            gpus_requested: spec.gpus,
            gpus_assigned,
        };

        // Add to executed keys cache for dedupe
        self.executed_keys.insert(spec.idempotency_key.clone());

        let original_name = task_path.file_name().unwrap().to_string_lossy();
        let result_name = if original_name.ends_with(".json") {
             original_name.replace(".json", ".result.json")
        } else {
             format!("{}.result.json", original_name)
        };
        
        let result_path = done_dir.join(&result_name);
        lfs::atomic_write_json(&result_path, &result)?;

        // Move claimed file to done (as archive of the request)
        // Or just delete it? 
        // Doc says: "move claimed task file into done (optional, for audit)"
        // Let's move it.
        let archived_task_path = done_dir.join(task_path.file_name().unwrap());
        std::fs::rename(task_path, &archived_task_path)?;

                // Clear heartbeat running task

                self.update_heartbeat(None).await?;

        

                Ok(())

            }

        }

        

        #[cfg(test)]

        mod tests {

            use super::*;

            use tempfile::tempdir;

            use leaseq_core::models::TaskSpec;

            use uuid::Uuid;

        

            #[tokio::test]

            async fn test_poll_and_claim() -> Result<()> {

                let dir = tempdir()?;

                let root = dir.path().to_path_buf();

                let node = "test-node".to_string();

                

                let inbox = root.join("inbox").join(&node);

                let claimed = root.join("claimed").join(&node);

                lfs::ensure_dir(&inbox)?;

                lfs::ensure_dir(&claimed)?;

                

                // Create a dummy task

                let task_file = inbox.join("001_T1_uuid.json");

                let spec = TaskSpec {

                    task_id: "T1".to_string(),

                    idempotency_key: "k1".to_string(),

                    lease_id: models::LeaseId("test-lease".to_string()),

                    target_node: node.clone(),

                    seq: 1,

                    uuid: Uuid::new_v4(),

                    created_at: time::OffsetDateTime::now_utc(),

                    cwd: "/tmp".to_string(),

                    env: std::collections::HashMap::new(),

                    gpus: 0,

                    command: "echo test".to_string(),

                };

                lfs::atomic_write_json(&task_file, &spec)?;

                

                let runner = Runner {

                    _lease_id: "test-lease".to_string(),

                    node: node.clone(),

                    root: root.clone(),

                    executed_keys: std::collections::HashSet::new(),

                };

                

                let claimed_path = runner.poll_and_claim().await?.expect("Should claim task");

                assert!(claimed_path.exists());

                assert!(claimed_path.to_str().unwrap().contains("claimed"));

                assert!(!task_file.exists());

                

                Ok(())

            }

        }

        