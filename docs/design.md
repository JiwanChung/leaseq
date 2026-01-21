# leaseq — Persistent Leases and Resilient Task Execution for Local & Slurm

## motivation

`leaseq` is designed for researchers who need to:

* run **sequential or batched experiments without releasing compute**
* survive **SSH disconnects**
* work on **shared filesystems with NFSv4 semantics**
* unify **local (pueue-like)** and **Slurm** workflows
* reliably track **stdout/stderr per experiment**

Existing tools fail due to shared-FS locking, daemon locality, or Slurm interaction assumptions.
`leaseq` is intentionally **boring, explicit, and resilient**.

---

## core abstractions

### lease

A **lease** is an execution backend with reserved capacity.

Two types:

* **local lease** (always on, default)
* **Slurm lease** (explicit, resource-holding job)

### task

A **task** is a single experiment command that:

* executes exactly once
* owns one stdout and one stderr
* runs inside a lease

### runner

A **runner** executes tasks for a lease.

* local lease: one runner per host
* Slurm lease: one runner per node

---

## key design decisions (locked)

* local lease is **always available**
* local is the **default backend**
* Slurm lease is **explicit opt-in**
* `gpus = 0` by default → CPU jobs first-class
* lease ID:

  * local: `local:<hostname>`
  * Slurm: **Slurm jobid**
* stdout/stderr are owned by `leaseq`, not Slurm
* shared FS treated as **eventually consistent (NFSv4)**

---

## high-level architecture

```
user CLI
  │
  │ (writes commands, reads status)
  ▼
shared FS (archive)
~/.leaseq/
  index.json
  runs/<lease_id>/
    inbox/ claimed/ done/ ack/
    events/ hb/
    logs/
  ▲
  │ (atomic rename + polling)
  ▼
runner(s)
  │
  │ (exec child processes)
  ▼
tasks
```

**No sockets. No locks. No sqlite. No RPC.**

---

## local lease (default, always on)

### invariant

On every host:

* there exists `lease_id = local:<hostname>`
* runner is always available
* capacity is auto-detected

### capacity detection

* GPUs: `nvidia-smi -L | wc -l` → `0` if unavailable
* CPUs: `os.cpu_count()` (informational)

### runtime storage (node-local)

To avoid NFS lag for live execution:

```
$XDG_RUNTIME_DIR/leaseq/local:<host>/
  inbox/ claimed/ done/ ack/ hb/ logs/
```

Fallback:

```
/tmp/leaseq/$UID/local:<host>/
```

### archive storage (shared FS)

After task completion:

```
~/.leaseq/runs/local:<host>/logs/
```

---

## Slurm lease (explicit)

### create Slurm lease

```bash
leaseq lease create --slurm \
  --nodes 4 \
  --time 12:00:00 \
  --partition gpu \
  --gpus-per-node 8 \
  --qos high
```

* submits a **keeper job** (`sbatch`)
* keeper job:

  * allocates resources
  * launches one runner per node
  * sleeps until canceled
* printed jobid **is the lease ID**

### release Slurm lease

```bash
leaseq lease release <jobid>
```

Equivalent to `scancel`.

---

## lease tracking

### index (fast UX)

`~/.leaseq/index.json`

```json
{
  "default_lease": "local:myhost",
  "leases": {
    "local:myhost": {},
    "99887766": {
      "created_at": 1736380000,
      "name": "expA"
    }
  }
}
```

### authoritative state

* `~/.leaseq/runs/<lease_id>/meta/lease.json`
* Slurm (`squeue`, `scontrol`)
* heartbeat files
* task directories

Index is rebuildable.

---

## task submission

### default (local lease, CPU job)

```bash
leaseq add -- python preprocess.py
```

### request GPUs (local or Slurm)

```bash
leaseq add --gpus 1 -- python train.py
leaseq add --gpus all -- python ddp.py
```

### Slurm lease, node-explicit

```bash
leaseq add --lease 99887766 --node nodeA -- python train.py
```

### auto placement

```bash
leaseq add --place spread -- python job.py
```

---

## task file (mailbox)

Path:

```
inbox/<node>/000012_T014_<uuid>.json
```

Content:

```json
{
  "task_id": "T014",
  "idempotency_key": "local:myhost-000012",
  "command": "python train.py",
  "cwd": "/path",
  "env": {},
  "gpus": 0,
  "created_at": 1736381200
}
```

Publication rule:

* write temp → `rename()`
* immutable after publish

---

## runner execution model

### default: exclusive-per-node

* one task at a time per node
* task may use all GPUs if requested
* avoids GPU binding complexity

### execution

```bash
bash -lc '
  cd <cwd>
  <env>
  <command>
' > logs/T014.out 2> logs/T014.err
```

---

## handshake & resilience (NFSv4-safe)

### atomic state transitions

* `inbox → claimed → done`
* all via `rename()`

### idempotency

* runner checks `done/` for `idempotency_key`
* duplicates are skipped safely

### acknowledgements

* runner writes `ack/<task>.ack.json` on claim
* CLI may retry if ack not seen

### assignment liveness

* `add` command checks heartbeat timestamp before assigning
* Rejects nodes with stale heartbeats (>120s) to prevent black-holing tasks

### heartbeats

* `hb/<node>.json` updated every 5s via a **background thread**
* Ensures liveness even during blocking task execution
* stale (>120s) = runner unhealthy / dead

---

## stdout / stderr tracking

### invariant

Each task owns:

* `logs/T014.out`
* `logs/T014.err`

### properties

* no interleaving
* survives SSH disconnect
* survives crashes
* independent of Slurm logging

### optional hardening

* write logs to `/tmp`
* `fsync`
* atomic rename to shared FS on completion

---

## inspecting output

### show logs

```bash
leaseq logs --task T014
leaseq logs --task T014 --stderr
leaseq logs --task T014 --tail 200
```

### follow (live)

```bash
leaseq follow
```

Behavior:

* follows stdout of the **single running task**
* if multiple running tasks:

  * prompts for `--task` or `--node`
* if none running:

  * suggests last finished task

Variants:

```bash
leaseq follow --task T014
leaseq follow --node nodeA
leaseq follow --stderr
leaseq follow --all
```

---

## status & inspection

```bash
leaseq status
leaseq tasks --state running
leaseq tasks --state failed
leaseq lease ls
leaseq lease use <lease_id>
```

Example status:

```
local:myhost
RUNNING  T014  python train.py
PENDING  T015  python sweep.py
```

---

## Slurm flag support

### first-class

* `--partition`
* `--qos`
* `--account`
* `--constraint`
* `--reservation`
* `--nodes`
* `--time`
* `--gpus-per-node`

### passthrough

```bash
--sbatch-arg "--gres=gpu:8"
--sbatch-arg "--comment=leaseq"
```

Exact args stored in `meta/lease.json`.

---

## failure behavior

### SSH disconnect

* no effect
* runners continue
* logs preserved

### filesystem lag

* eventual visibility
* no duplication
* no corruption

### runner crash

* other nodes unaffected
* logs preserved
* **Zombie Recovery**: On restart, runner scans `claimed/`, moves any found tasks back to `inbox/` for safe retry.
* heartbeat marks failure (stale timestamp)

---

## guarantees

* compute is never released between tasks
* tasks execute **at most once**
* stdout/stderr preserved per task
* works on NFSv4 gateways
* local and Slurm unified
* CPU jobs first-class
* zero user-managed tmux required

---

## non-goals

* replacing Slurm scheduling
* distributed consensus
* sqlite / shared locks
* speculative execution

---

## summary

**leaseq** unifies local and Slurm execution under a single, resilient abstraction:

> *A lease is reserved capacity; a task is an exactly-once command with owned logs.*

By leaning on atomic filesystem semantics instead of daemons or locks, `leaseq` remains robust under real HPC conditions — including NFSv4 lag, multi-node execution, and SSH disconnects.
