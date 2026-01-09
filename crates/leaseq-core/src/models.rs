use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LeaseId(pub String);

impl std::fmt::Display for LeaseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "lease_type", rename_all = "lowercase")]
pub enum LeaseMeta {
    Local {
        lease_id: LeaseId,
        #[serde(with = "time::serde::timestamp")]
        created_at: OffsetDateTime,
        local: LocalLeaseConfig,
    },
    Slurm {
        lease_id: LeaseId,
        name: Option<String>,
        #[serde(with = "time::serde::timestamp")]
        created_at: OffsetDateTime,
        slurm: SlurmLeaseConfig,
        mode: ExecutionMode,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalLeaseConfig {
    pub total_gpus: u32,
    pub parallel: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlurmLeaseConfig {
    pub sbatch_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionMode {
    #[default]
    ExclusivePerNode,
    Fractional,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    pub task_id: String,
    pub idempotency_key: String,
    pub lease_id: LeaseId,
    pub target_node: String,
    pub seq: u64,
    pub uuid: Uuid,
    #[serde(with = "time::serde::timestamp")]
    pub created_at: OffsetDateTime,
    pub cwd: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub gpus: u32, // 0 for CPU, >0 for GPU
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: String,
    pub idempotency_key: String,
    pub node: String,
    #[serde(with = "time::serde::timestamp")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::timestamp")]
    pub finished_at: OffsetDateTime,
    pub exit_code: i32,
    pub stdout: String, // path relative to run dir
    pub stderr: String, // path relative to run dir
    pub runtime_s: f64,
    #[serde(default)]
    pub command: String, // Original command for reference
    #[serde(default)]
    pub gpus_requested: u32, // GPUs requested
    #[serde(default)]
    pub gpus_assigned: String, // Actual GPU IDs assigned (e.g., "0,1" or "0,1,2,3")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub node: String,
    #[serde(with = "time::serde::timestamp")]
    pub ts: OffsetDateTime,
    pub running_task_id: Option<String>,
    pub pending_estimate: u32,
    pub runner_pid: u32,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Event {
    Claimed { task_id: String, node: String },
    Started { task_id: String, node: String },
    Finished { task_id: String, exit_code: i32 },
    Failed { task_id: String, error: String },
    SkippedDup { task_id: String, key: String },
    Cancelled { task_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_lease_id_display() {
        let local = LeaseId("local:myhost".to_string());
        assert_eq!(format!("{}", local), "local:myhost");

        let slurm = LeaseId("12345".to_string());
        assert_eq!(format!("{}", slurm), "12345");
    }

    #[test]
    fn test_task_spec_serialization() {
        let spec = TaskSpec {
            task_id: "T001".to_string(),
            idempotency_key: "key-001".to_string(),
            lease_id: LeaseId("local:myhost".to_string()),
            target_node: "myhost".to_string(),
            seq: 1,
            uuid: uuid::Uuid::nil(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            cwd: "/home/user".to_string(),
            env: HashMap::new(),
            gpus: 0,
            command: "echo hello".to_string(),
        };

        let json = serde_json::to_string(&spec).unwrap();
        let parsed: TaskSpec = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.task_id, "T001");
        assert_eq!(parsed.command, "echo hello");
    }

    #[test]
    fn test_task_result_serialization() {
        let result = TaskResult {
            task_id: "T001".to_string(),
            idempotency_key: "key-001".to_string(),
            node: "myhost".to_string(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            finished_at: OffsetDateTime::UNIX_EPOCH,
            exit_code: 0,
            stdout: "logs/T001.out".to_string(),
            stderr: "logs/T001.err".to_string(),
            runtime_s: 10.5,
            command: "echo hello".to_string(),
            gpus_requested: 2,
            gpus_assigned: "0,1".to_string(),
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: TaskResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.task_id, "T001");
        assert_eq!(parsed.exit_code, 0);
        assert_eq!(parsed.command, "echo hello");
        assert_eq!(parsed.gpus_requested, 2);
        assert_eq!(parsed.gpus_assigned, "0,1");
    }

    #[test]
    fn test_heartbeat_serialization() {
        let hb = Heartbeat {
            node: "myhost".to_string(),
            ts: OffsetDateTime::UNIX_EPOCH,
            running_task_id: Some("T001".to_string()),
            pending_estimate: 5,
            runner_pid: 12345,
            version: "0.1.0".to_string(),
        };

        let json = serde_json::to_string(&hb).unwrap();
        let parsed: Heartbeat = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.node, "myhost");
        assert_eq!(parsed.running_task_id, Some("T001".to_string()));
    }

    #[test]
    fn test_event_serialization() {
        let event = Event::Finished {
            task_id: "T001".to_string(),
            exit_code: 0,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("FINISHED"));

        let parsed: Event = serde_json::from_str(&json).unwrap();
        match parsed {
            Event::Finished { task_id, exit_code } => {
                assert_eq!(task_id, "T001");
                assert_eq!(exit_code, 0);
            }
            _ => panic!("Expected Finished event"),
        }
    }

    #[test]
    fn test_lease_meta_local_serialization() {
        let meta = LeaseMeta::Local {
            lease_id: LeaseId("local:myhost".to_string()),
            created_at: OffsetDateTime::UNIX_EPOCH,
            local: LocalLeaseConfig {
                total_gpus: 8,
                parallel: 1,
            },
        };

        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("\"lease_type\":\"local\""));

        let parsed: LeaseMeta = serde_json::from_str(&json).unwrap();
        match parsed {
            LeaseMeta::Local { lease_id, local, .. } => {
                assert_eq!(lease_id.0, "local:myhost");
                assert_eq!(local.total_gpus, 8);
            }
            _ => panic!("Expected Local lease meta"),
        }
    }
}
