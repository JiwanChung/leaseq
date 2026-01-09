use directories::ProjectDirs;
use std::path::PathBuf;
use std::env;

pub fn leaseq_home_dir() -> PathBuf {
    if let Ok(p) = env::var("LEASEQ_HOME") {
        return PathBuf::from(p);
    }
    
    if let Some(_proj_dirs) = ProjectDirs::from("", "", "leaseq") {
        // ProjectDirs uses ~/.config/leaseq on Linux usually, but we want ~/.leaseq for compatibility/simplicity as per design doc? 
        // Design doc says `~/.leaseq/`. 
        // Let's force ~/.leaseq if possible, or just use home dir.
        // Actually ProjectDirs::data_dir() might be appropriate but often that's ~/.local/share/leaseq.
        // Design doc explicitly says `~/.leaseq/`.
        let home = directories::UserDirs::new().expect("Could not find user home directory");
        return home.home_dir().join(".leaseq");
    }
    
    // Fallback
    PathBuf::from(".leaseq")
}

pub fn runtime_dir() -> PathBuf {
    if let Ok(p) = env::var("LEASEQ_RUNTIME_DIR") {
        return PathBuf::from(p);
    }

    if let Some(runtime) = ProjectDirs::from("", "", "leaseq").and_then(|p| p.runtime_dir().map(|p| p.to_path_buf())) {
        return runtime;
    }

    // Fallback to /tmp/leaseq/$UID
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/leaseq/{}", uid))
}

pub fn local_lease_id() -> String {
    let hostname = hostname::get().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|_| "localhost".to_string());
    format!("local:{}", hostname)
}
