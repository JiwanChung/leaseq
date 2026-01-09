# leaseq TUI specification

## goals

* Provide a fast, low-friction view of:

  * available leases (local + Slurm)
  * node/runners health
  * queued/running/finished/failed tasks
  * stdout/stderr following (`follow` equivalent)
* Must work even when:

  * SSH disconnects (Slurm leases keep running)
  * shared FS visibility lags (NFSv4)
  * some runners are down
* Must not require RPC/sockets; read-only from:

  * `~/.leaseq/index.json`
  * `~/.leaseq/runs/<lease_id>/...` (archive)
  * local runtime dir for local lease (optional)
  * Slurm CLI (`squeue`, `scontrol`) only for lease state, not task state

---

## entrypoint

* `leaseq tui` (default: open on default lease)
* `leaseq tui --lease <id>` (open directly on a lease)
* `leaseq tui --local` (open local lease `local:<hostname>`)
* `leaseq tui --slurm` (open lease list, filter slurm)

---

## overall layout

### main screen: split view (3-pane)

```
┌──────────────────────── leases (left) ────────────────────────┐
│ local:myhost   RUNNING  gpus=8  runner=OK   *default          │
│ 99887766       RUNNING  nodes=4 gpn=8  left=10:42             │
│ 11223344       PENDING  nodes=2 gpn=0  reason=Resources       │
└───────────────────────────────────────────────────────────────┘
┌──────────── nodes/runners (top-right) ───────────┬─ task list ┐
│ nodeA OK  hb=2s  running=T014  pending=1         │ T014 RUN   │
│ nodeB OK  hb=5s  running=-     pending=2         │ T015 PEND  │
│ nodeC STALE hb=95s running=?   pending=0         │ T012 FAIL  │
│ nodeD OK  hb=1s  running=T011  pending=0         │ ...        │
└──────────────────────────────────────────────────┴────────────┘
┌──────────────────────────── log viewer (bottom) ──────────────┐
│ [stdout] T014  nodeA  python train.py --cfg a.yaml             │
│ ... tail output ...                                            │
└───────────────────────────────────────────────────────────────┘
status bar: lease=<id>  filters=state:RUN  refresh=2s  fs=ok  q:quit
```

Pane focus cycles with `Tab`.

---

## data sources and refresh model

### task truth (no Slurm dependency)

Task state derived from directories:

* PENDING: file present in `inbox/<node>/`
* RUNNING: file present in `claimed/<node>/`
* DONE: result present in `done/<node>/...result.json`

### heartbeat/liveness

* `hb/<node>.json` contains `ts`, `running_task_id`, etc.
* Node state:

  * OK if `now - hb.ts <= hb_stale_s` (default 60s)
  * STALE otherwise

### lease state

* local lease: always RUNNING (unless local runner disabled)
* slurm lease: `squeue -j <jobid>` / `scontrol show job <jobid>`

  * show RUNNING/PENDING/COMPLETED/CANCELLED/TIMEOUT
  * show time left if available

### refresh cadence

* Default UI refresh: every 2s
* Directory scans:

  * inbox/claimed/done: every 2–5s (adaptive backoff)
  * hb: every 2s
* Slurm queries:

  * every 10–30s (configurable) to avoid load

### NFS lag tolerance

* UI must treat “missing” as “unknown” rather than “gone” for short windows:

  * if a task disappears from `claimed/` but no `done/` result yet:

    * mark as `LOST?` and display a warning icon until confirmed
* Always keep last-seen snapshot in memory for smooth UI.

---

## screens

### screen 1: lease list (default landing if no default lease)

* Table columns:

  * lease id (jobid or local:host)
  * type (local/slurm)
  * state (RUNNING/PENDING/etc.)
  * capacity summary (nodes, gpn; or local gpus)
  * time left (slurm)
  * name (optional)
* Actions:

  * `Enter`: open selected lease dashboard
  * `n`: create slurm lease (opens form)
  * `x`: release selected lease (slurm only; confirmation)

### screen 2: lease dashboard (main split view)

* Left: leases
* Top-right: nodes/runners
* Middle-right: task list
* Bottom: log viewer (optional; collapsible)

### screen 3: task details (modal)

Shows:

* task id, node, state
* command, cwd, env (collapsed)
* timestamps: created/claimed/finished
* exit code (if any)
* log paths
* quick actions: follow stdout/stderr, open logs, copy command

### screen 4: slurm lease create (form)

Fields:

* name (optional)
* nodes, time
* partition, qos, account, constraint, reservation
* gpu spec: gpus-per-node (default 0)
* extra sbatch args (repeatable)
  Preview pane shows generated `sbatch` command and keeper script path.

---

## keybindings

### global

* `q`: quit
* `?`: help overlay
* `Tab` / `Shift+Tab`: change pane focus
* `/`: search (context-dependent)
* `r`: refresh now
* `p`: pause auto-refresh (toggle)
* `c`: copy selected value (task id, command, etc.)

### lease list pane

* `j/k` or arrows: move selection
* `Enter`: open lease
* `l`: set selected lease as default (`lease use`)
* `x`: release lease (slurm only) with confirmation
* `f`: filter (local/slurm/running/pending)

### nodes pane

* `Enter`: filter task list to that node
* `s`: show node stats (hb age, running task, pending count)

### task list pane

* `Enter`: open task details modal
* `F`: follow (stdout) for selected task
* `E`: follow stderr for selected task
* `A`: follow both (stdout+stderr merged)
* `L`: open logs view (non-follow, paged)
* `X`: cancel task (writes cancel command file; see below)
* `R`: retry task (re-enqueue with same command; new idempotency key)
* `1/2/3/4`: quick filters (pending/running/failed/finished)

### log viewer pane

* `Space`: toggle follow (tail -f mode)
* `PgUp/PgDn`: scroll
* `g/G`: top/bottom
* `o`: switch stdout↔stderr
* `m`: merge/unmerge stdout+stderr
* `esc`: collapse log pane

---

## `follow` semantics in TUI

`leaseq follow` CLI becomes:

* `F` (follow stdout) in task list
* If no task selected:

  * if exactly one RUNNING task exists: follow it
  * else open a selector listing RUNNING tasks

In dashboard, a dedicated “Follow” action exists:

* `f`: follow “current running” based on the same rule above.

---

## task cancellation / control model (no RPC)

Because runners poll FS, task control is file-based:

### cancel

UI writes:

* `control/<node>/cancel_T014_<uuid>.json`

Runner checks control inbox each loop and:

* if T014 running: send SIGTERM, then SIGKILL after grace period
* if pending: remove from inbox and mark CANCELLED result

### retry

UI writes a new task file (new seq and new idempotency key) with same command.

### pause lane (node)

UI writes:

* `control/<node>/pause.json`
  Runner stops claiming new tasks but allows current task to finish.

### resume lane

* `control/<node>/resume.json`

(These are optional but very useful.)

---

## filtering and sorting

### filters

* by state: PENDING/RUNNING/FAILED/FINISHED/LOST?
* by node
* by substring match on command
* by “has stderr output” (non-empty err file or exit != 0)

### sorting

* default: state priority (RUNNING > PENDING > FAILED > FINISHED), then start time desc
* optional: by seq number within node lane

---

## configuration (TUI-relevant)

Config keys (stored in `~/.leaseq/config.json`):

* `ui.refresh_s` (default 2)
* `ui.slurm_refresh_s` (default 20)
* `ui.hb_stale_s` (default 60)
* `ui.follow_auto_latest` (default false)
* `ui.max_tasks_display` (default 5000)
* `ui.log_tail_lines` (default 200)

---

## performance considerations

* Avoid expensive recursive scans; prefer:

  * `os.scandir()` per directory
  * incremental diffing vs previous snapshot
* Shard directories per node to keep dir sizes manageable (already in spec).
* Cap visible task list; offer paging/search for huge histories.
* Slurm calls must be rate-limited.

---

## implementation notes (non-binding)

Any of these stacks fits:

* Python: **Textual** (recommended), Rich, or Urwid
* Go: Bubble Tea
* Rust: Ratatui

Log following:

* implement as “tail with polling” rather than relying on inotify (NFS).
* for local runtime dirs, inotify is OK but not necessary.

---

## minimal MVP for TUI (must-have)

1. lease list view with state
2. lease dashboard: nodes + task list
3. per-task stdout/stderr view + follow
4. filters: state + node
5. heartbeat-based node health
6. slurm lease create/release actions (optional for MVP, but valuable)
