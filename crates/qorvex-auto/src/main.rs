mod ast;
mod converter;
mod error;
mod executor;
mod parser;
mod runtime;

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

use qorvex_core::ipc::{qorvex_dir, IpcServer};
use qorvex_core::session::Session;
use qorvex_core::simctl::Simctl;

use crate::converter::LogConverter;
use crate::error::AutoError;
use crate::executor::ScriptExecutor;

#[derive(Parser)]
#[command(name = "qorvex-auto", about = "Script-based automation runner for iOS Simulator")]
struct Cli {
    #[arg(short, long, default_value = "default", env = "QORVEX_SESSION")]
    session: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a .qvx automation script
    Run {
        /// Path to the script file (reads stdin if omitted)
        script: Option<PathBuf>,
    },
    /// Convert an action log (JSONL) to a .qvx script
    Convert {
        /// Path to the JSONL log file (reads stdin if omitted)
        log: Option<PathBuf>,
        /// Name for the output script file
        #[arg(short, long)]
        name: Option<String>,
        /// Print to stdout instead of saving to file
        #[arg(long)]
        stdout: bool,
    },
}

fn automation_log_dir() -> PathBuf {
    let dir = qorvex_dir().join("automation").join("logs");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn automation_scripts_dir() -> PathBuf {
    let dir = qorvex_dir().join("automation").join("scripts");
    std::fs::create_dir_all(&dir).ok();
    dir
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Run { script } => run_script(script, &cli.session).await,
        Command::Convert { log, name, stdout } => convert_log(log, name, stdout),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        process::exit(e.exit_code());
    }
}

async fn run_script(script_path: Option<PathBuf>, session_name: &str) -> Result<(), AutoError> {
    // Read the script source and determine base directory for includes
    let (source, base_dir) = match script_path {
        Some(ref path) => {
            let src = std::fs::read_to_string(path).map_err(|e| AutoError::Io(e))?;
            let dir = path.canonicalize()
                .unwrap_or_else(|_| path.clone())
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            (src, dir)
        }
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            (buf, std::env::current_dir().unwrap_or_default())
        }
    };

    // Parse
    let script = parser::parse(&source)?;

    // Get booted simulator (best-effort, scripts can use use_device)
    let simulator_udid = Simctl::get_booted_udid().ok();

    // Create session with automation log directory
    let session = Session::new_with_log_dir(
        simulator_udid.clone(),
        session_name,
        automation_log_dir(),
    );

    // Spawn IPC server so qorvex-live can connect
    let ipc_session = session.clone();
    let ipc_name = session_name.to_string();
    let ipc_handle = tokio::spawn(async move {
        let server = IpcServer::new(ipc_session, &ipc_name);
        let _ = server.run().await;
    });

    // Execute with automatic session lifecycle
    let mut script_executor = ScriptExecutor::new(session.clone(), simulator_udid, base_dir);

    session.log_action(
        qorvex_core::action::ActionType::StartSession,
        qorvex_core::action::ActionResult::Success,
        None,
        None,
    ).await;

    let result = script_executor.execute_script(&script).await;

    session.log_action(
        qorvex_core::action::ActionType::EndSession,
        qorvex_core::action::ActionResult::Success,
        None,
        None,
    ).await;

    // Brief delay for event delivery to connected watchers
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Cleanup
    script_executor.cleanup();
    ipc_handle.abort();

    result
}

fn convert_log(
    log_path: Option<PathBuf>,
    name: Option<String>,
    to_stdout: bool,
) -> Result<(), AutoError> {
    let script_text = match log_path {
        Some(ref path) => LogConverter::convert_file(path)?,
        None => LogConverter::convert_stdin()?,
    };

    if to_stdout {
        print!("{}", script_text);
    } else {
        let script_name = name.unwrap_or_else(|| {
            log_path
                .as_ref()
                .and_then(|p| p.file_stem())
                .and_then(|s| s.to_str())
                .unwrap_or("converted")
                .to_string()
        });
        let output_path = automation_scripts_dir().join(format!("{}.qvx", script_name));
        std::fs::write(&output_path, &script_text)?;
        eprintln!("Saved to {}", output_path.display());
    }

    Ok(())
}
