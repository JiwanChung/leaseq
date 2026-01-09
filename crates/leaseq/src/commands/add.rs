use anyhow::{Result, Context};
use leaseq_core::{fs as lfs, models, config};
use uuid::Uuid;
use std::env;

pub async fn run(command: Vec<String>, lease: Option<String>, node: Option<String>) -> Result<()> {
    add_task(command.join(" "), lease, node).await
}

pub async fn add_task(command: String, lease: Option<String>, node: Option<String>) -> Result<()> {
    let lease_id = lease.unwrap_or_else(config::local_lease_id);
    
    // Resolve root
    let root = if lease_id.starts_with("local:") {
        config::runtime_dir().join(&lease_id)
    } else {
        config::leaseq_home_dir().join("runs").join(&lease_id)
    };

    let target_node = if let Some(n) = node {
        n
    } else if lease_id.starts_with("local:") {
        // Local lease -> local node
        hostname::get()?.to_string_lossy().into_owned()
    } else {
        // Slurm lease -> pick a node from heartbeats
        let hb_dir = root.join("hb");
        let files = lfs::list_files_sorted(&hb_dir).unwrap_or_default();
        if let Some(f) = files.first() {
            f.file_stem().unwrap().to_string_lossy().into_owned()
        } else {
            return Err(anyhow::anyhow!("No active nodes found for lease {}. Please specify --node.", lease_id));
        }
    };

    // Create TaskSpec
    let task_uuid = Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();
    let unix_micros = (now.unix_timestamp_nanos() / 1000) as u64;
    
    let task_id = format!("T{}", &task_uuid.simple().to_string()[..6]);
    
    let spec = models::TaskSpec {
        task_id: task_id.clone(),
        idempotency_key: format!("{}-{}-{}", lease_id, target_node, unix_micros),
        lease_id: models::LeaseId(lease_id.clone()),
        target_node: target_node.clone(),
        seq: unix_micros, 
        uuid: task_uuid,
        created_at: now,
        cwd: env::current_dir()?.to_string_lossy().into_owned(),
        env: env::vars().collect(),
        gpus: 0,
        command: command.clone(),
    };

    let filename = format!("{:016}_{}_{}.json", unix_micros, task_id, task_uuid);
    let inbox_path = root.join("inbox").join(&target_node).join(filename);

    lfs::atomic_write_json(&inbox_path, &spec).context("Failed to write task")?;
    
    // println!("Submitted task {} to lease {} node {}", task_id, lease_id, target_node);
    Ok(())
}
