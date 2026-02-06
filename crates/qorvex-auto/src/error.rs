use std::fmt;

#[derive(Debug)]
pub enum AutoError {
    Parse { message: String, line: usize },
    Runtime { message: String, line: usize },
    ActionFailed { message: String, line: usize },
    Io(std::io::Error),
}

impl AutoError {
    pub fn exit_code(&self) -> i32 {
        match self {
            AutoError::Parse { .. } => 2,
            AutoError::Runtime { .. } => 3,
            AutoError::ActionFailed { .. } => 1,
            AutoError::Io(_) => 4,
        }
    }
}

impl fmt::Display for AutoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AutoError::Parse { message, line } => write!(f, "Parse error at line {}: {}", line, message),
            AutoError::Runtime { message, line } => write!(f, "Runtime error at line {}: {}", line, message),
            AutoError::ActionFailed { message, line } => write!(f, "Action failed at line {}: {}", line, message),
            AutoError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for AutoError {}

impl From<std::io::Error> for AutoError {
    fn from(e: std::io::Error) -> Self {
        AutoError::Io(e)
    }
}
