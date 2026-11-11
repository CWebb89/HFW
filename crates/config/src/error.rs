//! Profile errors.

use std::fmt;

/// Everything a profile can get wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A line had no `=`.
    MissingEquals {
        /// One-based line number.
        line: usize,
    },
    /// A numeric setting held something that is not a finite number.
    NotANumber {
        /// Setting name.
        key: String,
        /// Text found instead of a number.
        value: String,
    },
    /// A boolean setting held something other than true/false/yes/no/1/0.
    NotABoolean {
        /// Setting name.
        key: String,
        /// Text found instead of a boolean.
        value: String,
    },
    /// An enumerated setting held a value outside its allowed set.
    BadChoice {
        /// Setting name.
        key: &'static str,
        /// Text found.
        value: String,
        /// Comma-separated allowed values.
        allowed: &'static str,
    },
    /// A setting name that the profile does not define.
    UnknownKey {
        /// One-based line number.
        line: usize,
        /// Setting name.
        key: String,
    },
    /// A `<<TAG` block was never closed.
    UnclosedBlock {
        /// One-based line number of the key.
        line: usize,
        /// Setting name.
        key: String,
        /// Terminating tag that was expected.
        tag: String,
    },
    /// A block was closed but never assigned to a setting.
    UnknownBlock {
        /// Setting name.
        key: String,
    },
    /// A value parsed but is outside the range the engine accepts.
    OutOfRange {
        /// Setting name.
        key: &'static str,
        /// What the value must satisfy.
        rule: &'static str,
        /// Value found.
        value: String,
    },
    /// A profile file could not be read.
    Read {
        /// Path as given on the command line.
        path: String,
        /// Underlying operating system message.
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::MissingEquals { line } => {
                write!(f, "line {line}: expected 'key = value'")
            }
            ConfigError::NotANumber { key, value } => {
                write!(f, "'{key}' needs a number, got '{value}'")
            }
            ConfigError::NotABoolean { key, value } => {
                write!(f, "'{key}' needs true or false, got '{value}'")
            }
            ConfigError::BadChoice {
                key,
                value,
                allowed,
            } => {
                write!(f, "'{key}' must be one of {allowed}, got '{value}'")
            }
            ConfigError::UnknownKey { line, key } => {
                write!(f, "line {line}: unknown setting '{key}'")
            }
            ConfigError::UnclosedBlock { line, key, tag } => {
                write!(f, "line {line}: block '{key}' is never closed with {tag}")
            }
            ConfigError::UnknownBlock { key } => write!(f, "unknown block setting '{key}'"),
            ConfigError::OutOfRange { key, rule, value } => {
                write!(f, "'{key}' must be {rule}, got {value}")
            }
            ConfigError::Read { path, message } => write!(f, "cannot read {path}: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {}
