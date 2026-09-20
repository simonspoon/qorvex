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
    /// Option (flag) specifications for the command.
    pub options: &'static [OptionSpec],
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
    /// Complete with installed app bundle IDs.
    BundleId,
    /// No special completion (free text).
    None,
}

/// Specification for a command option (flag).
#[derive(Debug, Clone)]
pub struct OptionSpec {
    /// The flag name (e.g. "--label").
    pub flag: &'static str,
    /// Whether this flag takes a value (e.g. --timeout takes a number).
    pub takes_value: bool,
    /// Short description for the completion popup.
    pub description: &'static str,
}

/// Shared `--platform` option for device/agent commands.
const PLATFORM_OPTION: OptionSpec = OptionSpec {
    flag: "--platform",
    takes_value: true,
    description: "Target platform: ios (default) or android",
};

/// All available REPL commands.
pub static COMMANDS: &[CommandDef] = &[
    // Session commands
    CommandDef {
        name: "start-session",
        description: "Start a new session",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "end-session",
        description: "End the current session",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "get-session-info",
        description: "Get session information",
        args: &[],
        options: &[],
    },
    // Device commands
    CommandDef {
        name: "list-devices",
        description: "List available devices (--platform ios|android)",
        args: &[],
        options: &[PLATFORM_OPTION],
    },
    CommandDef {
        name: "list-physical-devices",
        description: "List connected physical devices",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "use-device",
        description: "Select a simulator by UDID",
        args: &[ArgSpec {
            name: "udid",
            completion: ArgCompletion::DeviceUdid,
        }],
        options: &[],
    },
    CommandDef {
        name: "boot-device",
        description: "Boot a device (--platform ios|android)",
        args: &[ArgSpec {
            name: "udid",
            completion: ArgCompletion::DeviceUdid,
        }],
        options: &[PLATFORM_OPTION],
    },
    CommandDef {
        name: "start-agent",
        description: "Build/launch agent (--platform ios|android)",
        args: &[ArgSpec {
            name: "project_dir",
            completion: ArgCompletion::None,
        }],
        options: &[PLATFORM_OPTION],
    },
    CommandDef {
        name: "stop-agent",
        description: "Stop managed agent process",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "set-target",
        description: "Set target app bundle ID",
        args: &[ArgSpec {
            name: "bundle_id",
            completion: ArgCompletion::BundleId,
        }],
        options: &[],
    },
    CommandDef {
        name: "get-target-info",
        description: "Get target app metadata",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "memory-info",
        description: "Report target app and device memory",
        args: &[],
        options: &[OptionSpec {
            flag: "--tag",
            takes_value: true,
            description: "Annotate the action log entry with a free-text tag",
        }],
    },
    CommandDef {
        name: "start-target",
        description: "Launch the target application",
        args: &[],
        options: &[OptionSpec {
            flag: "--force",
            takes_value: false,
            description: "Relaunch even if the app is already running",
        }],
    },
    CommandDef {
        name: "stop-target",
        description: "Terminate the target application",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "set-timeout",
        description: "Set default wait timeout (ms)",
        args: &[ArgSpec {
            name: "ms",
            completion: ArgCompletion::None,
        }],
        options: &[],
    },
    // Device / app management commands (all act on the selected device)
    CommandDef {
        name: "create-device",
        description: "Create a simulator and select it",
        args: &[
            ArgSpec {
                name: "name",
                completion: ArgCompletion::None,
            },
            ArgSpec {
                name: "device_type",
                completion: ArgCompletion::None,
            },
            ArgSpec {
                name: "runtime",
                completion: ArgCompletion::None,
            },
        ],
        options: &[],
    },
    CommandDef {
        name: "wait-for-boot",
        description: "Wait until the selected device finishes booting",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "shutdown-device",
        description: "Shut down the selected device",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "delete-device",
        description: "Delete the selected device",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "quiet-device",
        description: "Quiet the selected device's mediaanalysisd",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "install-app",
        description: "Install an app bundle",
        args: &[ArgSpec {
            name: "path",
            completion: ArgCompletion::None,
        }],
        options: &[],
    },
    CommandDef {
        name: "uninstall-app",
        description: "Uninstall an app",
        args: &[ArgSpec {
            name: "bundle_id",
            completion: ArgCompletion::BundleId,
        }],
        options: &[],
    },
    CommandDef {
        name: "app-container",
        description: "Print an app container path",
        args: &[
            ArgSpec {
                name: "bundle_id",
                completion: ArgCompletion::BundleId,
            },
            ArgSpec {
                name: "container",
                completion: ArgCompletion::None,
            },
        ],
        options: &[],
    },
    CommandDef {
        name: "list-apps",
        description: "List installed apps",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "add-media",
        description: "Add photos/videos to the device libraries",
        args: &[ArgSpec {
            name: "file",
            completion: ArgCompletion::None,
        }],
        options: &[],
    },
    CommandDef {
        name: "open-url",
        description: "Open a URL on the device",
        args: &[ArgSpec {
            name: "url",
            completion: ArgCompletion::None,
        }],
        options: &[],
    },
    CommandDef {
        name: "grant-permission",
        description: "Grant/revoke/reset an app's access to a service",
        args: &[
            ArgSpec {
                name: "verb",
                completion: ArgCompletion::None,
            },
            ArgSpec {
                name: "service",
                completion: ArgCompletion::None,
            },
            ArgSpec {
                name: "bundle_id",
                completion: ArgCompletion::BundleId,
            },
        ],
        options: &[],
    },
    CommandDef {
        name: "set-appearance",
        description: "Switch the device between light and dark mode",
        args: &[ArgSpec {
            name: "appearance",
            completion: ArgCompletion::None,
        }],
        options: &[],
    },
    CommandDef {
        name: "set-content-size",
        description: "Set the Dynamic Type content size",
        args: &[ArgSpec {
            name: "size",
            completion: ArgCompletion::None,
        }],
        options: &[],
    },
    CommandDef {
        name: "device-log",
        description: "Show recent unified-log output",
        args: &[],
        options: &[
            OptionSpec {
                flag: "--last",
                takes_value: true,
                description: "How far back to read (e.g. 5m, 1h)",
            },
            OptionSpec {
                flag: "--predicate",
                takes_value: true,
                description: "NSPredicate to filter log entries",
            },
        ],
    },
    // Screen commands
    CommandDef {
        name: "get-screenshot",
        description: "Capture a screenshot",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "get-screen-info",
        description: "Get UI hierarchy as JSON",
        args: &[],
        options: &[],
    },
    // UI commands
    CommandDef {
        name: "list-elements",
        description: "List all UI elements",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "tap",
        description: "Tap an element",
        args: &[ArgSpec {
            name: "selector",
            completion: ArgCompletion::ElementSelector,
        }],
        options: &[
            OptionSpec {
                flag: "--label",
                takes_value: false,
                description: "Match by label instead of ID",
            },
            OptionSpec {
                flag: "--type",
                takes_value: true,
                description: "Filter by element type",
            },
            OptionSpec {
                flag: "--no-wait",
                takes_value: false,
                description: "Skip retry, attempt once",
            },
            OptionSpec {
                flag: "--timeout",
                takes_value: true,
                description: "Wait timeout in ms",
            },
        ],
    },
    CommandDef {
        name: "swipe",
        description: "Swipe the screen",
        args: &[ArgSpec {
            name: "direction",
            completion: ArgCompletion::None,
        }],
        options: &[],
    },
    CommandDef {
        name: "tap-location",
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
        options: &[],
    },
    CommandDef {
        name: "get-value",
        description: "Get an element's value",
        args: &[ArgSpec {
            name: "selector",
            completion: ArgCompletion::ElementSelector,
        }],
        options: &[
            OptionSpec {
                flag: "--label",
                takes_value: false,
                description: "Match by label instead of ID",
            },
            OptionSpec {
                flag: "--type",
                takes_value: true,
                description: "Filter by element type",
            },
            OptionSpec {
                flag: "--no-wait",
                takes_value: false,
                description: "Skip retry, attempt once",
            },
        ],
    },
    CommandDef {
        name: "wait-for",
        description: "Wait for element to appear",
        args: &[ArgSpec {
            name: "selector",
            completion: ArgCompletion::ElementSelector,
        }],
        options: &[
            OptionSpec {
                flag: "--label",
                takes_value: false,
                description: "Match by label instead of ID",
            },
            OptionSpec {
                flag: "--type",
                takes_value: true,
                description: "Filter by element type",
            },
            OptionSpec {
                flag: "--timeout",
                takes_value: true,
                description: "Wait timeout in ms",
            },
        ],
    },
    CommandDef {
        name: "wait-for-not",
        description: "Wait for element to disappear",
        args: &[ArgSpec {
            name: "selector",
            completion: ArgCompletion::ElementSelector,
        }],
        options: &[
            OptionSpec {
                flag: "--label",
                takes_value: false,
                description: "Match by label instead of ID",
            },
            OptionSpec {
                flag: "--type",
                takes_value: true,
                description: "Filter by element type",
            },
            OptionSpec {
                flag: "--timeout",
                takes_value: true,
                description: "Wait timeout in ms",
            },
        ],
    },
    // Input commands
    CommandDef {
        name: "send-keys",
        description: "Send keyboard input",
        args: &[ArgSpec {
            name: "text",
            completion: ArgCompletion::None,
        }],
        options: &[],
    },
    CommandDef {
        name: "log-comment",
        description: "Log a comment to session",
        args: &[ArgSpec {
            name: "message",
            completion: ArgCompletion::None,
        }],
        options: &[],
    },
    // General commands
    CommandDef {
        name: "help",
        description: "Show help message",
        args: &[],
        options: &[],
    },
    CommandDef {
        name: "quit",
        description: "Exit the REPL",
        args: &[],
        options: &[],
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
    candidates.sort_by_key(|c| std::cmp::Reverse(c.score));
    candidates
}
