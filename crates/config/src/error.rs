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
    /// A setting's value did not parse as what it claims to be.
    BadValue {
        /// Setting name.
        key: &'static str,
        /// Why the value was rejected.
        why: String,
    },
    /// A profile file could not be read.
    Read {
        /// Path as given on the command line.
        path: String,
        /// Underlying operating system message.
        message: String,
    },
    /// A preset's `inherits` chain points back at itself.
    RecursivePreset {
        /// Files visited, in order.
        chain: Vec<String>,
    },
    /// A file that holds several presets in `[print:…]` sections rather than one
    /// preset. Exporting the single preset is the fix.
    SectionedPreset {
        /// One-based line number of the first section header.
        line: usize,
        /// The section header, as written.
        section: String,
    },
}

impl ConfigError {
    /// Names the setting the profile actually wrote, for an error raised under
    /// this crate's own key after an alias was resolved.
    ///
    /// The range and choice errors keep this crate's key: their name field is a
    /// compile-time constant, and reporting `walls` for a bad `perimeters` value
    /// is still the setting the user needs to look at.
    pub(crate) fn instead_of(self, written: &str) -> ConfigError {
        match self {
            ConfigError::NotANumber { value, .. } => ConfigError::NotANumber {
                key: written.to_owned(),
                value,
            },
            ConfigError::NotABoolean { value, .. } => ConfigError::NotABoolean {
                key: written.to_owned(),
                value,
            },
            ConfigError::UnknownKey { line, .. } => ConfigError::UnknownKey {
                line,
                key: written.to_owned(),
            },
            other => other,
        }
    }
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
            ConfigError::BadValue { key, why } => write!(f, "'{key}' is not usable: {why}"),
            ConfigError::Read { path, message } => write!(f, "cannot read {path}: {message}"),
            ConfigError::RecursivePreset { chain } => {
                write!(f, "preset inheritance loops: {}", chain.join(" -> "))
            }
            ConfigError::SectionedPreset { line, section } => write!(
                f,
                "line {line}: {section} is a preset bundle section, not a single preset \
                 -- export the preset itself"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}
