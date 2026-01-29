<p align="center">
  <h1 align="center">leaseq</h1>
  <p align="center">
    <strong>Persistent leases & resilient task execution for Local & Slurm HPC</strong>
  </p>
  <p align="center">
    <a href="#installation"><img src="https://img.shields.io/badge/rust-1.70+-orange.svg" alt="Rust 1.70+"></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
    <a href="#features"><img src="https://img.shields.io/badge/NFS-safe-green.svg" alt="NFS Safe"></a>
  </p>
</p>

---

**leaseq** is a lightweight task queue for researchers who need to run sequential experiments without releasing compute. It survives SSH disconnects, works on shared filesystems (NFSv4), and unifies local and Slurm workflows.

```
No sockets. No locks. No sqlite. No RPC.
Just atomic file renames on a shared filesystem.
```

## Why leaseq?

| Problem | leaseq Solution |
|---------|-----------------|
| SSH disconnect kills your jobs | Tasks persist on filesystem, runners continue |
| Slurm releases nodes between jobs | Hold a "lease" - nodes stay allocated |
| NFS locking is unreliable | Atomic renames only, no locks needed |
| Different tools for local vs cluster | Same CLI everywhere |
| Lost stdout/stderr | Every task owns dedicated log files |

## Features

- **Local-first**: Works immediately on any machine, no setup required
- **Slurm integration**: Hold nodes with persistent leases across experiments
- **NFS-safe**: Designed for shared filesystems with eventual consistency
- **Rich TUI**: Real-time monitoring with vim-style navigation
- **GPU-aware**: Track GPU requests and assignments per task
- **Resilient**: Survives disconnects, restarts, and network issues
- **Simple**: No daemons to configure, no databases to manage

## Installation

### From crates.io (coming soon)

```bash
cargo install leaseq
```

### From GitHub

```bash
cargo install --git https://github.com/JiwanChung/leaseq.git
```

### From source

```bash
git clone https://github.com/JiwanChung/leaseq.git
cd leaseq
cargo build --release
cp target/release/leaseq ~/.local/bin/
```

### Requirements

- Rust 1.70+
- For Slurm: `sbatch`, `scancel` in PATH

## Quick Start

### Local Mode (Default)

```bash
# Start the local daemon (runs in background)
leaseq daemon start

# Submit tasks to the local queue
leaseq submit -- python train.py --epochs 100
leaseq submit -- python train.py --epochs 200 --lr 0.001

# Monitor with TUI
leaseq tui

# Or check status from CLI
leaseq status
leaseq tasks
leaseq logs <task_id>
```

### Slurm Mode

```bash
# Allocate an interactive lease (mimics salloc but persistent)
leaseq add -- --partition=gpu --gres=gpu:1 --time=4:00:00
# (Waits for job start, then auto-connects you to a shell on the node)

# Disconnect? Re-attach later:
leaseq shell

# Submit tasks to the existing Slurm lease (queue mode)
leaseq submit --lease <jobid> -- python distributed_train.py
leaseq submit --lease <jobid> -- python eval.py --checkpoint best.pt

# Monitor
leaseq tui --lease <jobid>

# Release nodes when done
leaseq lease release <jobid>
```

## TUI

The terminal UI provides real-time monitoring of your tasks:

```
┌─────────────────────── LeaseQ Monitor | Lease: 12345 ───────────────────────┐
├──────────────────┬────────────────────────────────┬─────────────────────────┤
│ Nodes            │ Tasks [Recent]                 │ Detail                  │
├──────────────────┼────────────────────────────────┼─────────────────────────┤
│ gpu-node-01 [OK] │ T03a11e8 RUNNING G4 gpu-node-01│ ID: T03a11e81           │
│ gpu-node-02 [OK] │ T69f9065 RUNNING G2 gpu-node-02│ State: RUNNING          │
│ gpu-node-03 [STALE] │ T9c55996 STUCK   G1 gpu-node-03│ Node: gpu-node-03       │
│                  │ Te8c767a FAILED  G2 gpu-node-03│ GPUs: 4 [0,1,2,3]       │
│                  │ T04fa34d DONE    G1 gpu-node-02│ Exit: -                 │
│                  │                                │                         │
│                  │                                │ Command:                │
│                  │                                │ python train.py --dist  │
├──────────────────┴────────────────────────────────┴─────────────────────────┤
│ Logs: T03a11e81 (stdout) [FOLLOW]                                           │
├─────────────────────────────────────────────────────────────────────────────┤
│ [2024-01-09 10:00:35] Starting epoch 1/50                                   │
│ [2024-01-09 10:01:00] Epoch 1 - Batch 100/5005 - Loss: 6.234                │
│ [2024-01-09 10:02:00] Epoch 1 - Batch 200/5005 - Loss: 5.891                │
├─────────────────────────────────────────────────────────────────────────────┤
│ h/j/k/l:Nav | Enter:Actions | z:Zoom | F:Filter | a:Add | q:Quit | ?:Help   │
└─────────────────────────────────────────────────────────────────────────────┘
```

### TUI Features

- **Visual Health**: Stale nodes and stuck tasks are highlighted
- **Task Actions**: Press `Enter` on a task to View Logs, **Recover to Inbox**, or Cancel
- **Filters**: Quickly filter by Running, Pending, Done, Failed, or **Stuck**

### TUI Keybindings

| Key | Action |
|-----|--------|
| `h/j/k/l` | Navigate panes and lists |
| `Enter` | **Task Actions** / toggle zoom |
| `F` | Cycle filter (Recent/All/Running/Pending/Done/Failed/**Stuck**) |
| `z` | Maximize logs pane |
| `f` | Toggle follow mode (in zoomed logs) |
| `e` | Toggle stdout/stderr |
| `a` | Add new task (Submit) |
| `?` | Help |
| `q` | Quit |

## CLI Reference

```bash
# Allocations
leaseq add [SLURM_ARGS]                              # Allocate new lease & shell (e.g. leaseq add --partition=gpu)
leaseq lease release <ID>                            # Release/Cancel a lease
leaseq shell [--lease ID]                            # Open interactive shell in active lease

# Task Submission
leaseq submit [--lease ID] [--node NAME] -- <CMD>    # Submit a task to queue
leaseq cancel <TASK_ID>                              # Cancel a task

# Monitoring
leaseq status                                        # Show queue status
leaseq tasks [--state STATE]                         # List tasks (states: pending, running, done, failed, stuck)
leaseq logs <TASK_ID>                                # Show task logs
leaseq follow <TASK_ID>                              # Follow logs in real-time
leaseq tui [--lease ID]                              # Start TUI

# Daemon
leaseq daemon start                                  # Start local runner
leaseq daemon stop                                   # Stop local runner
leaseq daemon status                                 # Check daemon status
```

## Architecture

```
                          ┌─────────────┐
                          │   CLI/TUI   │
                          └──────┬──────┘
                                 │ writes tasks, reads status
                                 ▼
┌─────────────────────────────────────────────────────────────┐
│                    Shared Filesystem                         │
│  ~/.leaseq/                                                  │
│    ├── runs/<lease_id>/                                     │
│    │   ├── inbox/<node>/     ← pending tasks                │
│    │   ├── claimed/<node>/   ← running tasks                │
│    │   ├── done/<node>/      ← completed results            │
│    │   ├── hb/<node>.json    ← runner heartbeats            │
│    │   └── logs/             ← stdout/stderr files          │
│    └── index.json            ← lease registry               │
└─────────────────────────────────────────────────────────────┘
                                 ▲
                                 │ atomic rename + polling
                                 │
              ┌──────────────────┼──────────────────┐
              ▼                  ▼                  ▼
        ┌──────────┐       ┌──────────┐       ┌──────────┐
        │ Runner 1 │       │ Runner 2 │       │ Runner N │
        │ (node-01)│       │ (node-02)│       │ (node-N) │
        └──────────┘       └──────────┘       └──────────┘
```

### How It Works

1. **Tasks are files**: Each task is a JSON file in the `inbox/` directory
2. **Claiming is atomic**: Runners claim tasks via `rename()` to `claimed/`
3. **Results are persistent**: Completed tasks move to `done/` with exit codes and logs
4. **Heartbeats signal liveness**: Runners write periodic heartbeats to `hb/`
5. **No coordination needed**: Multiple runners can safely poll the same queue

## Project Structure

```
leaseq/
├── crates/
│   ├── leaseq-core/     # Shared models, filesystem utilities
│   └── leaseq/          # CLI, TUI, and task runner
└── docs/
    ├── design.md        # Architecture decisions
    └── impl.md          # Implementation notes
```

## Configuration

leaseq uses sensible defaults and requires minimal configuration:

```bash
# Environment variables
LEASEQ_HOME=~/.leaseq          # Data directory (default: ~/.leaseq)

# Local daemon settings are auto-detected:
# - Hostname for lease ID
# - Available GPUs via nvidia-smi
# - Parallel execution based on resources
```

## Comparison

| Feature | leaseq | pueue | Slurm | tmux+scripts |
|---------|--------|-------|-------|--------------|
| NFS-safe | ✅ | ❌ | ✅ | ✅ |
| Survives disconnect | ✅ | ✅ | ✅ | ❌ |
| Local + cluster | ✅ | ❌ | ❌ | ❌ |
| No daemon required | ✅ | ❌ | ✅ | ✅ |
| GPU tracking | ✅ | ❌ | ✅ | ❌ |
| Rich TUI | ✅ | ✅ | ❌ | ❌ |
| Hold nodes across jobs | ✅ | N/A | ❌ | N/A |

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

```bash
# Run tests
cargo test

# Build debug
cargo build

# Run TUI in development
cargo run --bin leaseq -- tui
```

## License

MIT License - see [LICENSE](LICENSE) for details.

---

<p align="center">
  Built for researchers who just want to run experiments.
</p>
