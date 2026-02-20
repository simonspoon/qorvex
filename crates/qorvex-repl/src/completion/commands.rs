//! Command definitions for the REPL.

use super::fuzzy::FuzzyFilter;
use super::{Candidate, CandidateKind};

/// A command definition with metadata for completion and help.
#[derive(Debug, Clone)]
pub struct CommandDef {
    /// The command name.
    pub name: &'static str,
    /// Short description shown in completion popup.
    pub description: &'static str,
    /// Argument specifications for the command.
    pub args: &'static [ArgSpec],
}

/// Specification for a command argument.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ArgSpec {
    /// Argument name for display.
    pub name: &'static str,
    /// What kind of completion to offer.
    pub completion: ArgCompletion,
}

/// What type of completion to offer for an argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ArgCompletion {
    /// Complete with element IDs from cached screen info.
    ElementId,
    /// Complete with element labels from cached screen info.
    ElementLabel,
    /// Smart element selector that auto-composes arguments (selector, label flag, type).
    ElementSelector,
    /// Complete with device UDIDs from cached device list.
    DeviceUdid,
    /// No special completion (free text).
    None,
}

/// All available REPL commands.
pub static COMMANDS: &[CommandDef] = &[
    // Session commands
    CommandDef {
        name: "start_session",
        description: "Start a new session",
        args: &[],
    },
    CommandDef {
        name: "end_session",
        description: "End the current session",
        args: &[],
    },
    CommandDef {
        name: "get_session_info",
        description: "Get session information",
        args: &[],
    },
    // Device commands
    CommandDef {
        name: "list_devices",
        description: "List available simulators",
        args: &[],
    },
    CommandDef {
        name: "use_device",
        description: "Select a simulator by UDID",
        args: &[ArgSpec {
            name: "udid",
            completion: ArgCompletion::DeviceUdid,
        }],
    },
    CommandDef {
        name: "boot_device",
        description: "Boot a simulator",
        args: &[ArgSpec {
            name: "udid",
            completion: ArgCompletion::DeviceUdid,
        }],
    },
    CommandDef {
        name: "start_agent",
        description: "Build/launch Swift agent",
        args: &[ArgSpec {
            name: "project_dir",
            completion: ArgCompletion::None,
        }],
    },
    CommandDef {
        name: "stop_agent",
        description: "Stop managed agent process",
        args: &[],
    },
    CommandDef {
        name: "set_target",
        description: "Set target app bundle ID",
        args: &[ArgSpec {
            name: "bundle_id",
            completion: ArgCompletion::None,
        }],
    },
    CommandDef {
        name: "set_timeout",
        description: "Set default wait timeout (ms)",
        args: &[ArgSpec {
            name: "ms",
            completion: ArgCompletion::None,
        }],
    },
    // Screen commands
    CommandDef {
        name: "get_screenshot",
        description: "Capture a screenshot",
        args: &[],
    },
    CommandDef {
        name: "get_screen_info",
        description: "Get UI hierarchy as JSON",
        args: &[],
    },
    CommandDef {
        name: "start_watcher",
        description: "Start screen change detection",
        args: &[ArgSpec {
            name: "interval_ms",
            completion: ArgCompletion::None,
        }],
    },
    CommandDef {
        name: "stop_watcher",
        description: "Stop screen change detection",
        args: &[],
    },
    // UI commands
    CommandDef {
        name: "list_elements",
        description: "List all UI elements",
        args: &[],
    },
    CommandDef {
        name: "tap",
        description: "Tap an element",
        args: &[ArgSpec {
            name: "selector",
            completion: ArgCompletion::ElementSelector,
        }],
    },
    CommandDef {
        name: "swipe",
        description: "Swipe the screen",
        args: &[ArgSpec {
            name: "direction",
            completion: ArgCompletion::None,
        }],
    },
    CommandDef {
        name: "tap_location",
        description: "Tap at screen coordinates",
        args: &[
            ArgSpec {
                name: "x",
                completion: ArgCompletion::None,
            },
            ArgSpec {
                name: "y",
                completion: ArgCompletion::None,
            },
        ],
    },
    CommandDef {
        name: "get_value",
        description: "Get an element's value",
        args: &[ArgSpec {
            name: "selector",
            completion: ArgCompletion::ElementSelector,
        }],
    },
    CommandDef {
        name: "wait_for",
        description: "Wait for element to appear",
        args: &[ArgSpec {
            name: "selector",
            completion: ArgCompletion::ElementSelector,
        }],
    },
    CommandDef {
        name: "wait_for_not",
        description: "Wait for element to disappear",
        args: &[ArgSpec {
            name: "selector",
            completion: ArgCompletion::ElementSelector,
        }],
    },
    // Input commands
    CommandDef {
        name: "send_keys",
        description: "Send keyboard input",
        args: &[ArgSpec {
            name: "text",
            completion: ArgCompletion::None,
        }],
    },
    CommandDef {
        name: "log_comment",
        description: "Log a comment to session",
        args: &[ArgSpec {
            name: "message",
            completion: ArgCompletion::None,
        }],
    },
    // General commands
    CommandDef {
        name: "help",
        description: "Show help message",
        args: &[],
    },
    CommandDef {
        name: "quit",
        description: "Exit the REPL",
        args: &[],
    },
];

/// Find a command by name.
pub fn find_command(name: &str) -> Option<&'static CommandDef> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// Get commands that match a prefix using fuzzy matching.
pub fn commands_matching(prefix: &str) -> Vec<Candidate> {
    let filter = FuzzyFilter::new();

    let mut candidates: Vec<Candidate> = COMMANDS
        .iter()
        .filter_map(|cmd| {
            let (score, indices) = filter.score(prefix, cmd.name)?;
            Some(Candidate {
                text: cmd.name.to_string(),
                description: cmd.description.to_string(),
                kind: CandidateKind::Command,
                score,
                match_indices: indices,
            })
        })
        .collect();

    // Sort by score descending
    candidates.sort_by(|a, b| b.score.cmp(&a.score));
    candidates
}
