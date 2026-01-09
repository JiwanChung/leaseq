use anyhow::Result;
use leaseq_core::{fs as lfs, models, config};

pub async fn run(lease: Option<String>) -> Result<()> {
    let lease_id = lease.unwrap_or_else(config::local_lease_id);
    
    let root = if lease_id.starts_with("local:") {
        config::runtime_dir().join(&lease_id)
    } else {
        config::leaseq_home_dir().join("runs").join(&lease_id)
    };
    
    println!("Lease: {}", lease_id);
    println!("Root:  {}", root.display());
    println!();

    // Read heartbeats
    let hb_dir = root.join("hb");
    let hb_files = lfs::list_files_sorted(&hb_dir).unwrap_or_default();
    println!("Nodes:");
    if hb_files.is_empty() {
        println!("  (none)");
    }
    for f in hb_files {
        if let Ok(hb) = lfs::read_json::<models::Heartbeat, _>(&f) {
            let age = (time::OffsetDateTime::now_utc() - hb.ts).as_seconds_f64();
            let status = if age > 60.0 { "STALE" } else { "OK" };
            println!("  {:<10} {} (seen {:.0}s ago) running={:?}", hb.node, status, age, hb.running_task_id);
        }
    }
    println!();

    // Read claimed (running)
    let claimed_dir = root.join("claimed");
    println!("Running Tasks:");
    if claimed_dir.exists() {
        for entry in std::fs::read_dir(&claimed_dir)? {
             let entry = entry?;
             if entry.path().is_dir() {
                 let node = entry.file_name();
                 for task_file in lfs::list_files_sorted(entry.path())? {
                     if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&task_file) {
                         println!("  {:<10} {:<10} {}", spec.task_id, node.to_string_lossy(), spec.command);
                     }
                 }
             }
        }
    }
    println!();

    // Read inbox (pending)
    let inbox_dir = root.join("inbox");
    println!("Pending Tasks:");
    if inbox_dir.exists() {
        for entry in std::fs::read_dir(&inbox_dir)? {
             let entry = entry?;
             if entry.path().is_dir() {
                 let node = entry.file_name();
                 for task_file in lfs::list_files_sorted(entry.path())? {
                     if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&task_file) {
                         println!("  {:<10} {:<10} {}", spec.task_id, node.to_string_lossy(), spec.command);
                     }
                 }
             }
        }
    }

    Ok(())
}
