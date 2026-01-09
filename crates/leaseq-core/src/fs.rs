use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn ensure_dir<P: AsRef<Path>>(path: P) -> io::Result<()> {
    fs::create_dir_all(path)
}

/// Write content to a file atomically by writing to a temp file first then renaming.
/// The temp file is created in the same directory to ensure atomic rename (same filesystem).
pub fn atomic_write_json<T: serde::Serialize, P: AsRef<Path>>(path: P, data: &T) -> io::Result<()> {
    let path = path.as_ref();
    let parent = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Path has no parent"))?;
    
    ensure_dir(parent)?;
    
    // Create temp file with unique name
    let temp_name = format!(".tmp.{}.{}", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"), Uuid::new_v4());
    let temp_path = parent.join(temp_name);
    
    {
        let mut file = File::create(&temp_path)?;
        let json = serde_json::to_string_pretty(data).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?; // Ensure durability
    }
    
    fs::rename(&temp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use serde::{Serialize, Deserialize};

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestData {
        name: String,
        value: i32,
    }

    #[test]
    fn test_atomic_write_read_json() -> io::Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("test.json");
        let data = TestData { name: "test".to_string(), value: 42 };

        atomic_write_json(&path, &data)?;
        let read: TestData = read_json(&path)?;

        assert_eq!(data, read);
        Ok(())
    }

    #[test]
    fn test_list_files_sorted() -> io::Result<()> {
        let dir = tempdir()?;
        let d = dir.path();

        File::create(d.join("002_task.json"))?;
        File::create(d.join("001_task.json"))?;
        File::create(d.join(".hidden"))?;

        let files = list_files_sorted(d)?;
        assert_eq!(files.len(), 2);
        assert!(files[0].to_str().unwrap().contains("001_task.json"));
        assert!(files[1].to_str().unwrap().contains("002_task.json"));

        Ok(())
    }

    #[test]
    fn test_list_files_sorted_empty_dir() -> io::Result<()> {
        let dir = tempdir()?;
        let files = list_files_sorted(dir.path())?;
        assert!(files.is_empty());
        Ok(())
    }

    #[test]
    fn test_list_files_sorted_nonexistent_dir() -> io::Result<()> {
        let files = list_files_sorted("/nonexistent/path/that/does/not/exist")?;
        assert!(files.is_empty());
        Ok(())
    }

    #[test]
    fn test_ensure_dir_creates_nested() -> io::Result<()> {
        let dir = tempdir()?;
        let nested = dir.path().join("a").join("b").join("c");
        ensure_dir(&nested)?;
        assert!(nested.exists());
        Ok(())
    }

    #[test]
    fn test_touch_creates_file() -> io::Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("touched.txt");
        assert!(!path.exists());
        touch(&path)?;
        assert!(path.exists());
        Ok(())
    }

    #[test]
    fn test_remove_file_if_exists() -> io::Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("to_remove.txt");

        // Should not error when file doesn't exist
        remove_file_if_exists(&path)?;

        // Create and then remove
        File::create(&path)?;
        assert!(path.exists());
        remove_file_if_exists(&path)?;
        assert!(!path.exists());

        Ok(())
    }

    #[test]
    fn test_atomic_write_creates_parent_dirs() -> io::Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("nested").join("deep").join("test.json");
        let data = TestData { name: "nested".to_string(), value: 100 };

        atomic_write_json(&path, &data)?;
        assert!(path.exists());

        let read: TestData = read_json(&path)?;
        assert_eq!(data, read);
        Ok(())
    }
}

/// Read JSON from a file
pub fn read_json<T: serde::de::DeserializeOwned, P: AsRef<Path>>(path: P) -> io::Result<T> {
    let file = File::open(path)?;
    let reader = io::BufReader::new(file);
    serde_json::from_reader(reader).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// List files in a directory sorted lexicographically (useful for task queues)
pub fn list_files_sorted<P: AsRef<Path>>(dir: P) -> io::Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    
    // Check if dir exists first
    if !dir.as_ref().exists() {
        return Ok(vec![]);
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && !path.file_name().unwrap().to_string_lossy().starts_with('.') {
            entries.push(path);
        }
    }
    
    entries.sort();
    Ok(entries)
}

pub fn touch<P: AsRef<Path>>(path: P) -> io::Result<()> {
    if path.as_ref().exists() {
        let _file = File::open(path.as_ref())?;
        // Update mtime? For now just opening is enough check, but to update mtime we might need more.
        // Actually touch usually means "create if not exists, else update time".
        // Rust std doesn't expose utime easily. 
        // For heartbeats we usually overwrite the content anyway.
        // If we just want to ensure it exists:
        return Ok(());
    }
    File::create(path)?;
    Ok(())
}

pub fn remove_file_if_exists<P: AsRef<Path>>(path: P) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
