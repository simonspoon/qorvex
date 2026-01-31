use std::process::Command;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AxeError {
    #[error("Command execution failed: {0}")]
    CommandFailed(String),
    #[error("axe tool not found - install with: brew install cameroncooke/axe/axe")]
    NotInstalled,
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// UI element from axe hierarchy dump
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UIElement {
    #[serde(rename = "AXUniqueId", default)]
    pub identifier: Option<String>,
    #[serde(rename = "AXLabel", default)]
    pub label: Option<String>,
    #[serde(rename = "AXValue", default)]
    pub value: Option<String>,
    #[serde(rename = "type", default)]
    pub element_type: Option<String>,
    #[serde(default)]
    pub frame: Option<ElementFrame>,
    #[serde(default)]
    pub children: Vec<UIElement>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub struct Axe;

impl Axe {
    /// Check if axe is installed
    pub fn is_installed() -> bool {
        Command::new("which")
            .arg("axe")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Dump UI hierarchy as JSON
    pub fn dump_hierarchy(udid: &str) -> Result<Vec<UIElement>, AxeError> {
        if !Self::is_installed() {
            return Err(AxeError::NotInstalled);
        }

        let output = Command::new("axe")
            .args(["describe-ui", "--udid", udid])
            .output()?;

        if !output.status.success() {
            return Err(AxeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }

        let hierarchy: Vec<UIElement> = serde_json::from_slice(&output.stdout)?;
        Ok(hierarchy)
    }

    /// Find element by identifier in hierarchy (recursive search)
    pub fn find_element(elements: &[UIElement], identifier: &str) -> Option<UIElement> {
        for element in elements {
            if element.identifier.as_deref() == Some(identifier) {
                return Some(element.clone());
            }
            if let Some(found) = Self::find_element(&element.children, identifier) {
                return Some(found);
            }
        }
        None
    }

    /// Tap at x,y coordinates
    pub fn tap(udid: &str, x: i32, y: i32) -> Result<(), AxeError> {
        if !Self::is_installed() {
            return Err(AxeError::NotInstalled);
        }

        let output = Command::new("axe")
            .args(["tap", "-x", &x.to_string(), "-y", &y.to_string(), "--udid", udid])
            .output()?;

        if !output.status.success() {
            return Err(AxeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }
        Ok(())
    }

    /// Tap element by identifier (uses axe --id flag)
    pub fn tap_element(udid: &str, identifier: &str) -> Result<(), AxeError> {
        if !Self::is_installed() {
            return Err(AxeError::NotInstalled);
        }

        let output = Command::new("axe")
            .args(["tap", "--id", identifier, "--udid", udid])
            .output()?;

        if !output.status.success() {
            return Err(AxeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string()
            ));
        }
        Ok(())
    }

    /// Get value of element by identifier
    pub fn get_element_value(udid: &str, identifier: &str) -> Result<Option<String>, AxeError> {
        let hierarchy = Self::dump_hierarchy(udid)?;
        let element = Self::find_element(&hierarchy, identifier)
            .ok_or_else(|| AxeError::CommandFailed(format!("Element '{}' not found", identifier)))?;
        Ok(element.value)
    }

    /// Flatten hierarchy to list of elements with identifiers
    pub fn list_elements(elements: &[UIElement]) -> Vec<UIElement> {
        let mut result = Vec::new();
        Self::collect_elements(elements, &mut result);
        result
    }

    fn collect_elements(elements: &[UIElement], result: &mut Vec<UIElement>) {
        for element in elements {
            if element.identifier.is_some() || element.label.is_some() {
                result.push(element.clone());
            }
            Self::collect_elements(&element.children, result);
        }
    }
}
