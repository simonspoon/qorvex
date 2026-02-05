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
    /// An element label.
    ElementLabel,
    /// Smart selector by element ID (auto-composed args).
    ElementSelectorById,
    /// Smart selector by element label (auto-composed args).
    ElementSelectorByLabel,
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
                        ArgCompletion::ElementLabel => {
                            element_label_candidates(&prefix, cached_elements)
                        }
                        ArgCompletion::ElementSelector => {
                            element_selector_candidates(&prefix, cached_elements, command.name)
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

/// Generate element label completion candidates with fuzzy matching.
fn element_label_candidates(prefix: &str, elements: &[UIElement]) -> Vec<Candidate> {
    let filter = FuzzyFilter::new();

    let mut candidates: Vec<Candidate> = elements
        .iter()
        .filter_map(|elem| {
            let label = elem.label.as_ref()?;
            if label.is_empty() {
                return None;
            }

            let (score, indices) = filter.score(prefix, label)?;

            let elem_type = elem.element_type.as_deref().unwrap_or("Unknown");
            let id_info = elem.identifier.as_deref().unwrap_or("");
            let description = if id_info.is_empty() {
                elem_type.to_string()
            } else {
                format!("{} [{}]", elem_type, id_info)
            };

            Some(Candidate {
                text: label.clone(),
                description,
                kind: CandidateKind::ElementLabel,
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

/// Quote a string if it contains characters that need escaping in command arguments.
fn quote_if_needed(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\'') || s.contains('(') || s.contains(')') {
        // Escape any existing double quotes and wrap in double quotes
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Generate smart element selector candidates that auto-compose command arguments.
///
/// Shows ALL elements (with identifier OR label) and composes appropriate arguments:
/// - Unique identifier → `"the-id"`
/// - Non-unique id + unique label → `"The Label", label`
/// - Non-unique id + non-unique label → `"The Label", label, Type`
/// - Non-unique id + no label → `"the-id", , Type`
/// - Unique label (no id) → `"The Label", label`
/// - Non-unique label (no id) → `"The Label", label, Type`
///
/// For `wait_for` command, label-based selectors include default timeout:
/// - By label → `"The Label", 5000, label`
fn element_selector_candidates(
    prefix: &str,
    elements: &[UIElement],
    command_name: &str,
) -> Vec<Candidate> {
    use std::collections::HashMap;

    let filter = FuzzyFilter::new();

    // Build uniqueness maps
    let mut id_counts: HashMap<&str, usize> = HashMap::new();
    let mut label_counts: HashMap<&str, usize> = HashMap::new();

    for elem in elements {
        if let Some(id) = elem.identifier.as_deref() {
            *id_counts.entry(id).or_insert(0) += 1;
        }
        if let Some(label) = elem.label.as_deref() {
            if !label.is_empty() {
                *label_counts.entry(label).or_insert(0) += 1;
            }
        }
    }

    let is_wait_for = command_name == "wait_for";

    let mut candidates: Vec<Candidate> = elements
        .iter()
        .filter_map(|elem| {
            let id = elem.identifier.as_deref();
            let label = elem.label.as_deref().filter(|l| !l.is_empty());
            let elem_type = elem.element_type.as_deref().unwrap_or("Unknown");

            // Skip elements with neither id nor label
            if id.is_none() && label.is_none() {
                return None;
            }

            // Determine uniqueness
            let id_is_unique = id.map(|i| id_counts.get(i) == Some(&1)).unwrap_or(false);
            let label_is_unique = label.map(|l| label_counts.get(l) == Some(&1)).unwrap_or(false);

            // Determine best selector strategy
            // Priority: unique ID > non-unique ID with label > unique label > ID + type > label + type
            let (text, kind) = if let Some(id) = id {
                if id_is_unique {
                    // Unique ID: just the selector
                    (id.to_string(), CandidateKind::ElementSelectorById)
                } else if let Some(label) = label {
                    // Non-unique ID but has label: use label-based selection
                    let quoted_label = quote_if_needed(label);
                    if label_is_unique {
                        let composed = if is_wait_for {
                            format!("{}, 5000, label", quoted_label)
                        } else {
                            format!("{}, label", quoted_label)
                        };
                        (composed, CandidateKind::ElementSelectorByLabel)
                    } else {
                        let composed = if is_wait_for {
                            format!("{}, 5000, label, {}", quoted_label, elem_type)
                        } else {
                            format!("{}, label, {}", quoted_label, elem_type)
                        };
                        (composed, CandidateKind::ElementSelectorByLabel)
                    }
                } else {
                    // Non-unique ID, no label: use type for disambiguation (best effort)
                    let composed = format!("{}, , {}", id, elem_type);
                    (composed, CandidateKind::ElementSelectorById)
                }
            } else if let Some(label) = label {
                let quoted_label = quote_if_needed(label);
                if label_is_unique {
                    // Unique label: selector, label flag
                    let composed = if is_wait_for {
                        // wait_for has timeout as arg 2, label as arg 3
                        format!("{}, 5000, label", quoted_label)
                    } else {
                        format!("{}, label", quoted_label)
                    };
                    (composed, CandidateKind::ElementSelectorByLabel)
                } else {
                    // Non-unique label: add type for disambiguation
                    let composed = if is_wait_for {
                        format!("{}, 5000, label, {}", quoted_label, elem_type)
                    } else {
                        format!("{}, label, {}", quoted_label, elem_type)
                    };
                    (composed, CandidateKind::ElementSelectorByLabel)
                }
            } else {
                return None;
            };

            // Try matching against identifier or label
            let id_match = id.and_then(|i| filter.score(prefix, i));
            let label_match = label.and_then(|l| filter.score(prefix, l));

            // Use the best match
            let (score, indices) = match (id_match, label_match) {
                (Some((id_score, id_indices)), Some((label_score, _))) => {
                    if id_score >= label_score {
                        (id_score, id_indices)
                    } else {
                        (label_score, Vec::new())
                    }
                }
                (Some(m), None) => m,
                (None, Some((score, _))) => (score, Vec::new()),
                (None, None) => return None,
            };

            // Build description
            let description = match (id, label) {
                (Some(_), Some(l)) => format!("{} \"{}\"", elem_type, l),
                (Some(_), None) => elem_type.to_string(),
                (None, Some(_)) => elem_type.to_string(),
                (None, None) => elem_type.to_string(),
            };

            Some(Candidate {
                text,
                description,
                kind,
                score,
                match_indices: indices,
            })
        })
        .collect();

    // Sort by score descending
    candidates.sort_by(|a, b| b.score.cmp(&a.score));
    candidates
}
