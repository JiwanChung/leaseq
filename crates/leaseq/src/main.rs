use clap::{Parser, Subcommand};
use anyhow::Result;
use std::path::PathBuf;

mod commands;
mod tui;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Add a task to the queue
    Add {
        #[arg(last = true, required = true)]
        command: Vec<String>,

        #[arg(long)]
        lease: Option<String>,

        #[arg(long)]
        node: Option<String>,
    },
    /// Show status
    Status {
        #[arg(long)]
        lease: Option<String>,
    },
    /// List tasks with filters
    Tasks {
        #[arg(long)]
        lease: Option<String>,

        /// Filter by state: all, pending, running, done, failed
        #[arg(long)]
        state: Option<String>,

        /// Filter by node
        #[arg(long)]
        node: Option<String>,

        /// Search in command or task ID
        #[arg(long)]
        search: Option<String>,
    },
    /// Show task logs
    Logs {
        /// Task ID
        task: String,

        #[arg(long)]
        lease: Option<String>,

        /// Show stderr instead of stdout
        #[arg(long)]
        stderr: bool,

        /// Show only the last N lines
        #[arg(long)]
        tail: Option<usize>,
    },
    /// Follow task output in real-time
    Follow {
        /// Task ID (auto-detects if single running task)
        #[arg(long)]
        task: Option<String>,

        #[arg(long)]
        lease: Option<String>,

        /// Filter to specific node
        #[arg(long)]
        node: Option<String>,

        /// Follow stderr instead of stdout
        #[arg(long)]
        stderr: bool,
    },
    /// Cancel a task
    Cancel {
        /// Task ID to cancel
        task: String,

        #[arg(long)]
        lease: Option<String>,
    },
    /// Manage the local runner daemon
    #[command(subcommand)]
    Daemon(DaemonCommands),
    /// Start TUI
    Tui {
        #[arg(long)]
        lease: Option<String>,
    },
    /// Manage leases
    #[command(subcommand)]
    Lease(commands::lease::LeaseCommands),
    /// Run the task runner (used internally by daemon)
    Run {
        /// Lease ID (e.g., local:myhost or slurm jobid)
        #[arg(long)]
        lease: String,

        /// Node name (defaults to hostname)
        #[arg(long)]
        node: Option<String>,

        /// Root directory for execution (overrides default lookup)
        #[arg(long)]
        root: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DaemonCommands {
    /// Start the local runner daemon
    Start,
    /// Stop the local runner daemon
    Stop,
    /// Show daemon status
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Add { command, lease, node }) => {
            commands::add::run(command, lease, node).await
        }
        Some(Commands::Status { lease }) => {
            commands::status::run(lease).await
        }
        Some(Commands::Tasks { lease, state, node, search }) => {
            commands::tasks::run(lease, state, node, search).await
        }
        Some(Commands::Logs { task, lease, stderr, tail }) => {
            commands::logs::run(task, lease, stderr, tail).await
        }
        Some(Commands::Follow { task, lease, node, stderr }) => {
            commands::follow::run(task, lease, node, stderr).await
        }
        Some(Commands::Cancel { task, lease }) => {
            commands::cancel::run(task, lease).await
        }
        Some(Commands::Daemon(cmd)) => match cmd {
            DaemonCommands::Start => commands::daemon::start().await,
            DaemonCommands::Stop => commands::daemon::stop().await,
            DaemonCommands::Status => commands::daemon::status().await,
        },
        Some(Commands::Tui { lease }) => {
            tui::run(lease).await
        }
        Some(Commands::Lease(cmd)) => {
            commands::lease::run(cmd).await
        }
        Some(Commands::Run { lease, node, root }) => {
            tracing_subscriber::fmt::init();
            commands::run::run(commands::run::RunArgs { lease, node, root }).await
        }
        None => {
            // Default to TUI
            tui::run(None).await
        }
    }
}