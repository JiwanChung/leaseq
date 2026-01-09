# impl.md — leaseq implementation plan (Rust)

This document specifies a Rust implementation strategy for **leaseq**:
- always-on local lease (`local:<hostname>`) by default
- explicit Slurm leases (`lease_id == jobid`)
- NFSv4-safe file handshake (atomic rename, no locks, no sockets)
- per-task stdout/stderr capture
- CLI + TUI (Rust)

---

## 0) scope and invariants

### must-haves
- Local lease is always present and usable with zero setup.
- Slurm lease creation/release works with common flags + passthrough args.
- Tasks are queued via filesystem mailbox:
  - `inbox/<node>/...` → `claimed/<node>/...` → `done/<node>/...`
- Exactly-once execution via idempotency key + dedupe in `done/`.
- Task logs:
  - one `Txxx.out` and one `Txxx.err` per task
  - follow/tail functionality
- TUI: leases list, node health, task list, log viewer.

### non-goals (explicit)
- No distributed consensus.
- No sqlite/lockfiles.
- No inotify dependency (must work on NFS).
- No attempt to replace Slurm scheduler semantics.

---

## 1) project layout

Rust workspace (recommended):

```

leaseq/
Cargo.toml
crates/
leaseq-core/        # protocol, state, fs ops, models
leaseq-runner/      # runner binary logic (per-node, local or slurm)
leaseq-cli/         # main binary: CLI + TUI + lease create/release

```

Binary names:
- `leaseq` (main CLI + TUI)
- `leaseq-runner` (runner executable invoked by local daemon or Slurm job script)

---

## 2) crates and dependencies

### CLI / parsing
- `clap` (derive) — CLI parsing
- `clap_complete` (optional) — completions

### JSON, time, ids
- `serde`, `serde_json`
- `chrono` or `time` (prefer `time` for lightweight)
- `uuid` (task uuid suffix)

### filesystem and atomic writes
- std `fs` + `tempfile` (for temp files) OR implement manual temp naming
- `fs2` (optional) for `sync_all` helpers; **do not rely on file locks**

### process execution
- `tokio` + `tokio::process` (async execution and tail follow)
  - alternatively sync `std::process` for MVP; async helps log follow

### TUI
- `ratatui` (formerly tui-rs)
- `crossterm`
- `tui-textarea` (optional) for search/filter inputs

### Slurm integration
- execute Slurm CLI tools: `sbatch`, `squeue`, `scontrol`, `scancel`, `srun`
- parse outputs robustly:
  - `squeue -h -o ...` with stable formatting
  - `scontrol show job -o` for key=value fields
- no Slurm API bindings required

### logging / tracing
- `tracing`, `tracing-subscriber`

---

## 3) directories and paths

### shared archive root
Default:
- `~/.leaseq/`

Subdirs:
- `~/.leaseq/index.json`
- `~/.leaseq/runs/<lease_id>/...`

### local runtime root (fast, node-local)
Prefer:
- `$XDG_RUNTIME_DIR/leaseq/`
Fallback:
- `/tmp/leaseq/<uid>/`

Local runtime for the always-on lease:
- `<runtime_root>/local:<hostname>/...`

### run directory structure (authoritative state)
For any lease id:

```

runs/<lease_id>/
meta/lease.json
inbox/<node>/
claimed/<node>/
ack/<node>/
done/<node>/
events/<node>.jsonl
hb/<node>.json
logs/

````

For local lease, live execution may occur in runtime root; completed artifacts are mirrored to archive root.

---

## 4) data model (serde)

### LeaseId
- Slurm lease: numeric jobid string, e.g. `"99887766"`
- Local lease: `"local:<hostname>"`

### LeaseMeta (`meta/lease.json`)
```json
{
  "lease_id": "99887766",
  "lease_type": "slurm",
  "name": "expA",
  "created_at": 1736380000,
  "slurm": {
    "sbatch_args": ["--nodes=4", "--time=12:00:00", "--partition=gpu", "--gpus-per-node=8"]
  },
  "mode": "exclusive-per-node"
}
````

For local:

```json
{
  "lease_id": "local:myhost",
  "lease_type": "local",
  "created_at": 1736380000,
  "local": {
    "total_gpus": 8,
    "parallel": 1
  }
}
```

### TaskSpec (command file in inbox)

```json
{
  "task_id": "T014",
  "idempotency_key": "99887766-nodeA-000012",
  "lease_id": "99887766",
  "target_node": "nodeA",
  "seq": 12,
  "uuid": "2b7c9c9b-...",
  "created_at": 1736381200,
  "cwd": "/path/to/project",
  "env": {"WANDB_MODE":"offline"},
  "gpus": 0,
  "command": "python train.py --cfg a.yaml"
}
```

### TaskResult (`done/<node>/*.result.json`)

```json
{
  "task_id": "T014",
  "idempotency_key": "99887766-nodeA-000012",
  "node": "nodeA",
  "started_at": 1736381300,
  "finished_at": 1736385021,
  "exit_code": 0,
  "stdout": "logs/T014.out",
  "stderr": "logs/T014.err",
  "runtime_s": 3721
}
```

### Heartbeat (`hb/<node>.json`)

```json
{
  "node": "nodeA",
  "ts": 1736382000,
  "running_task_id": "T014",
  "pending_estimate": 3,
  "runner_pid": 12345,
  "version": "0.1.0"
}
```

### Events (`events/<node>.jsonl`)

Append-only single-writer:

* `CLAIMED`, `STARTED`, `FINISHED`, `FAILED`, `SKIPPED_DUP`, `CANCELLED`, `LOST?`

---

## 5) filesystem protocol (NFSv4-safe)

### atomic write helper

Implement:

* write to temp file in same directory
* `sync_all` (best effort)
* `rename(temp, final)` — atomic publish/commit

Never modify files in place.

### task naming

In inbox:

* `000012_T014_<uuid>.json`

Sorting lexicographically gives FIFO per node.

### claim

Runner claims by atomic rename:

* `inbox/nodeA/X.json` → `claimed/nodeA/X.json`

### ack

Immediately after claim:

* write `ack/nodeA/T014.ack.json` (atomic publish)

### dedupe

Before execution, runner checks:

* if any `done/nodeA/*.result.json` contains same `idempotency_key`, skip execution and write event `SKIPPED_DUP`.

Implementation:

* store a node-local hashmap cache of executed keys, rebuilt from done dir at startup.

### lag tolerance

* polling only (no inotify)
* adaptive polling interval: 1–2s idle → backoff to 5–10s
* periodic full rescan: 30–60s

---

## 6) runner implementation (leaseq-runner)

### modes

Runner runs in two contexts:

* **local runner**: `target_node = hostname`, inbox is local runtime dir
* **slurm runner**: `target_node = hostname` (short), inbox is shared archive dir for that lease

Runner arguments:

* `--lease <lease_id>`
* `--node <hostname_short>`
* `--root <path>` (run dir root)
* `--mode exclusive-per-node|fractional` (MVP supports exclusive)

### main loop (exclusive-per-node)

Pseudo:

1. update heartbeat every 10s (atomic replace file)
2. poll inbox dir
3. choose smallest filename
4. claim via rename → claimed
5. write ack
6. parse TaskSpec; if malformed:

   * move to done with error result
7. dedupe check:

   * if already executed: write result `SKIPPED` and continue
8. execute:

   * open stdout/stderr files
   * spawn `bash -lc "<cd/env/cmd>"`
   * redirect stdout/stderr directly to files
9. write TaskResult and event
10. move claimed task file into done (optional, for audit)

### execution details

* Use `std::process::Command`:

  * `Command::new("bash").args(["-lc", script])`
* Set `current_dir` if possible; else `cd` in bash script (prefer `current_dir` + env vars in Command)
* Env merging:

  * runner adds `CUDA_VISIBLE_DEVICES` if allocating GPUs (later)
* Ensure stdout/stderr files created before spawn.

### cancellation (optional for MVP)

If implementing control files:

* `control/<node>/cancel_<taskid>_<uuid>.json`
* runner checks control dir each loop
* if cancel applies to running task:

  * send SIGTERM then SIGKILL after grace

---

## 7) local lease “always-on” runner

### goal

`leaseq add ...` works without starting anything manually.

### recommended approach: user systemd service (preferred)

Commands:

* `leaseq daemon enable`
* `leaseq daemon disable`
* `leaseq daemon status`

Implementation:

* write unit file into `~/.config/systemd/user/leaseq.service`
* `systemctl --user enable --now leaseq`
* suggest `loginctl enable-linger $USER` if truly always-on across reboots is desired (document only; do not auto-run without explicit request)

Fallback if systemd unavailable:

* `leaseq daemon start` uses `nohup leaseq-runner ... &` and stores pid in runtime dir.

### local run dir

Live mailbox/logs in runtime root:

* `$XDG_RUNTIME_DIR/leaseq/local:<host>/...`

Periodic mirroring (simple):

* on task completion, runner copies/renames logs + results into `~/.leaseq/runs/local:<host>/...`

---

## 8) CLI implementation (leaseq)

### command tree (clap)

* `leaseq add [--lease <id>] [--node <node>] [--place spread|local] [--gpus <n|all>] -- <cmd...>`
* `leaseq status [--lease <id>] [--wide]`
* `leaseq tasks [--lease <id>] [--state ...] [--node ...] [--search ...]`
* `leaseq logs --task <Tid> [--lease <id>] [--stderr] [--tail N]`
* `leaseq follow [--lease <id>] [--task <Tid>] [--node <node>] [--stderr] [--all]`
* `leaseq lease ls`
* `leaseq lease use <id>`
* `leaseq lease create --slurm ...` (explicit opt-in)
* `leaseq lease release <jobid>`
* `leaseq tui [--lease <id>]`

### default lease resolution

* default is `local:<hostname>` if present
* else last used in index
  Index file:
* `~/.leaseq/index.json` updated atomically

### `add` behavior

* Determine lease:

  * if `--lease` specified use it
  * else use default lease
* Determine placement:

  * local lease: only local node
  * slurm lease: if `--node` specified use it; else if `--place spread`, choose least-loaded node by:

    * reading hb pending estimate, or
    * counting inbox+claimed in that node dirs
* Create `TaskSpec` with `seq` per node:

  * seq can be derived by scanning highest seq in inbox/claimed/done for that node and incrementing (MVP)
  * better: keep `seq.json` per node updated atomically (later)
* Publish into `inbox/<node>/` via atomic rename

### `status` / `tasks`

* Read hb files to show liveness
* Read inbox/claimed/done to summarize counts
* For slurm leases: query `squeue/scontrol` for lease state/time left (rate-limited)

### `logs` and `follow`

* Find log path:

  * from `done` result if present
  * else compute `logs/<task_id>.out|err` by convention
* Tail:

  * implement polling tail (no inotify), refresh every 250–500ms for local, 1s for NFS
* `follow` selection:

  * if `--task` use it
  * else if `--node` follow running task on that node (claimed state)
  * else:

    * if exactly one running task in lease, follow it
    * if multiple, print list and ask user to specify (CLI) or open selector (TUI)
    * if none, print last finished suggestion

---

## 9) Slurm lease creation/release

### lease create (slurm)

`leaseq lease create --slurm ...`:

1. Build `sbatch` command using provided flags:

   * first-class: `--nodes`, `--time`, `--partition`, `--qos`, `--account`, `--constraint`, `--reservation`, `--gpus-per-node`
   * passthrough: repeated `--sbatch-arg`
2. Generate keeper script into a temp file:

   * script creates run dir `~/.leaseq/runs/$SLURM_JOB_ID/...`
   * writes `meta/lease.json`
   * launches one runner per node:

     * `srun --nodes=$SLURM_JOB_NUM_NODES --ntasks=$SLURM_JOB_NUM_NODES leaseq-runner --lease $SLURM_JOB_ID --node $(hostname -s) --root <run_dir>`
   * then `while true; do sleep 60; done`
3. Submit with `sbatch` and parse returned jobid:

   * use `sbatch --parsable` to get jobid reliably
4. Store in index; print jobid (lease_id)

### release

* `scancel <jobid>`

### state queries

* `squeue -h -j <jobid> -o "%T|%M|%L|%R"`
* fallback `scontrol show job -o <jobid>` parse key=val fields

---

## 10) TUI implementation (ratatui)

### screens

* Lease list (left)
* Lease dashboard:

  * nodes/runners table
  * tasks table
  * log viewer pane

### rendering model

* Snapshot loop every 2s:

  * read index + run dirs
  * read hb and task dirs
  * for slurm leases: update state at most every 20s

### interactions

* `Tab`: switch focus panes
* `Enter`: open lease or task detail modal
* `F`: follow stdout
* `E`: follow stderr
* `/`: search command substring
* `1/2/3/4`: filter by state

### log viewer

* implement a tail reader that:

  * maintains file offset
  * reads appended bytes
  * decodes as UTF-8 with replacement
* for NFS: polling tail (1s)
* for local runtime: faster polling (250ms)

---

## 11) testing plan

### unit tests

* atomic write + rename correctness (temp naming, content integrity)
* task filename parsing and ordering
* idempotency dedupe behavior

### integration tests (local)

* spawn local runner in temp dir
* enqueue tasks that print to stdout/stderr
* assert logs created and result written

### slurm integration (optional)

* mock `sbatch/squeue/scontrol/scancel` by injecting command runner trait
* parse outputs with fixture strings

### resilience tests

* simulate lag by delaying visibility (in tests: runner polls a “staging” dir)
* duplicate submissions with same idempotency key; verify exactly-once

---

## 12) implementation milestones (MVP-first)

### M1 (core local queue)

* always-on local lease (manual daemon start acceptable)
* inbox/claim/done, logs, results
* `add/status/logs/follow` CLI

### M2 (slurm lease)

* `lease create --slurm` with keeper job script
* runners on slurm nodes
* `lease ls/release/status`

### M3 (TUI)

* lease list + dashboard + follow
* basic filtering/search

### M4 (polish)

* systemd user service enable/disable
* task cancel/retry via control files
* local runtime mirroring policy configuration

---

## 13) notes on GPU handling (future-proof)

MVP:

* default `gpus=0`, treat tasks as CPU jobs
* allow `--gpus all` to set `CUDA_VISIBLE_DEVICES=0..N-1` (local only)

Later:

* implement GPU slot allocator per node:

  * maintain `gpu_alloc.json` in runtime dir (single-writer runner)
  * assign CUDA_VISIBLE_DEVICES for tasks requesting GPUs
* fractional scheduling mode can be added once allocation exists.

---

## 14) security and safety

* Avoid executing arbitrary strings without user intent:

  * `leaseq add -- <cmd...>` uses explicit delimiter
* Avoid writing outside run dir paths
* Use strict JSON parsing with error handling; malformed tasks go to `done` with failure record
* Ensure runner never deletes user data; only moves within leaseq directories.
