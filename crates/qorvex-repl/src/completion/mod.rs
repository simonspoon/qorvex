//! Completion logic for the REPL.

pub mod commands;
pub mod fuzzy;

use qorvex_core::axe::UIElement;
use qorvex_core::simctl::SimulatorDevice;

use self::commands::{find_command, commands_matching, ArgCompletion, CommandDef};
use self::fuzzy::FuzzyFilter;

/// The context in which completion is being performed.
#[derive(Debug, Clone)]
pub enum CompletionContext {
    /// Completing a command name.
    Command,
    /// Completing an argument to a command.
    Argument {
        command: &'static CommandDef,
        arg_index: usize,
    },
}

/// A completion candidate.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The text to insert.
    pub text: String,
    /// Description to show in the popup.
    pub description: String,
    /// Kind indicator for the popup.
    pub kind: CandidateKind,
    /// Match score (higher is better).
    pub score: i64,
    /// Indices of matched characters for highlighting.
    pub match_indices: Vec<usize>,
}

/// The type of completion candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    /// A command name.
    Command,
    /// An element identifier.
    ElementId,
    /// A device UDID.
    DeviceUdid,
}

/// State for the completion popup.
#[derive(Debug)]
pub struct CompletionState {
    /// Current candidates to show.
    pub candidates: Vec<Candidate>,
    /// Index of the selected candidate.
    pub selected: usize,
    /// Whether the completion popup is visible.
    pub visible: bool,
}

impl Default for CompletionState {
    fn default() -> Self {
        Self {
            candidates: Vec::new(),
            selected: 0,
            visible: false,
        }
    }
}

impl CompletionState {
    /// Move selection up.
    pub fn select_prev(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if !self.candidates.is_empty() {
            self.selected = (self.selected + 1).min(self.candidates.len() - 1);
        }
    }

    /// Get the currently selected candidate.
    pub fn selected_candidate(&self) -> Option<&Candidate> {
        self.candidates.get(self.selected)
    }

    /// Hide the completion popup.
    pub fn hide(&mut self) {
        self.visible = false;
        self.candidates.clear();
        self.selected = 0;
    }

    /// Update completions based on the current input.
    pub fn update(
        &mut self,
        input: &str,
        cached_elements: &[UIElement],
        cached_devices: &[SimulatorDevice],
    ) {
        let (context, prefix) = parse_completion_context(input);

        self.candidates = match context {
            CompletionContext::Command => {
                commands_matching(&prefix)
            }
            CompletionContext::Argument { command, arg_index } => {
                if let Some(arg_spec) = command.args.get(arg_index) {
                    match arg_spec.completion {
                        ArgCompletion::ElementId => {
                            element_candidates(&prefix, cached_elements)
                        }
                        ArgCompletion::DeviceUdid => {
                            device_candidates(&prefix, cached_devices)
                        }
                        ArgCompletion::None => Vec::new(),
                    }
                } else {
                    Vec::new()
                }
            }
        };

        self.visible = !self.candidates.is_empty();
        self.selected = 0;
    }
}

/// Parse the completion context from input.
fn parse_completion_context(input: &str) -> (CompletionContext, String) {
    // Check if we're inside parentheses (completing an argument)
    if let Some(paren_idx) = input.find('(') {
        let cmd_name = input[..paren_idx].trim();
        if let Some(cmd) = find_command(cmd_name) {
            let args_part = &input[paren_idx + 1..];
            // Count commas to determine which argument we're on
            let arg_index = args_part.matches(',').count();
            // Get the current argument prefix (text after last comma or opening paren)
            let prefix = args_part
                .rsplit(',')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();

            return (
                CompletionContext::Argument {
                    command: cmd,
                    arg_index,
                },
                prefix,
            );
        }
    }

    // Otherwise we're completing a command name
    (CompletionContext::Command, input.to_string())
}

/// Generate element ID completion candidates with fuzzy matching.
fn element_candidates(prefix: &str, elements: &[UIElement]) -> Vec<Candidate> {
    let filter = FuzzyFilter::new();

    let mut candidates: Vec<Candidate> = elements
        .iter()
        .filter_map(|elem| {
            let id = elem.identifier.as_ref()?;
            let label = elem.label.as_deref().unwrap_or("");

            // Try matching against identifier
            let id_match = filter.score(prefix, id);
            // Try matching against label
            let label_match = filter.score(prefix, label);

            // Use the best match
            let (score, indices) = match (id_match, label_match) {
                (Some((id_score, id_indices)), Some((label_score, _))) => {
                    if id_score >= label_score {
                        (id_score, id_indices)
                    } else {
                        // Use id indices even when label matched better,
                        // since we display the id
                        (label_score, Vec::new())
                    }
                }
                (Some(m), None) => m,
                (None, Some((score, _))) => (score, Vec::new()),
                (None, None) => return None,
            };

            let elem_type = elem.element_type.as_deref().unwrap_or("Unknown");
            let description = if label.is_empty() {
                elem_type.to_string()
            } else {
                format!("{} \"{}\"", elem_type, label)
            };

            Some(Candidate {
                text: id.clone(),
                description,
                kind: CandidateKind::ElementId,
                score,
                match_indices: indices,
            })
        })
        .collect();

    // Sort by score descending
    candidates.sort_by(|a, b| b.score.cmp(&a.score));
    candidates
}

/// Generate device UDID completion candidates with fuzzy matching.
fn device_candidates(prefix: &str, devices: &[SimulatorDevice]) -> Vec<Candidate> {
    let filter = FuzzyFilter::new();

    let mut candidates: Vec<Candidate> = devices
        .iter()
        .filter_map(|dev| {
            // Try matching against UDID
            let udid_match = filter.score(prefix, &dev.udid);
            // Try matching against name
            let name_match = filter.score(prefix, &dev.name);

            // Use the best match
            let (score, indices) = match (udid_match, name_match) {
                (Some((udid_score, udid_indices)), Some((name_score, _))) => {
                    if udid_score >= name_score {
                        (udid_score, udid_indices)
                    } else {
                        // Use empty indices when name matched better,
                        // since we display the UDID
                        (name_score, Vec::new())
                    }
                }
                (Some(m), None) => m,
                (None, Some((score, _))) => (score, Vec::new()),
                (None, None) => return None,
            };

            let description = format!("{} ({})", dev.name, dev.state);

            Some(Candidate {
                text: dev.udid.clone(),
                description,
                kind: CandidateKind::DeviceUdid,
                score,
                match_indices: indices,
            })
        })
        .collect();

    // Sort by score descending
    candidates.sort_by(|a, b| b.score.cmp(&a.score));
    candidates
}
