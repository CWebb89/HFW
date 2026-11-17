//! Intake for the shared PrusaSlicer/Orca configuration dialect.
//!
//! A preset written by PrusaSlicer or OrcaSlicer is a flat `key = value` file
//! with an `inherits` chain, and its keys are *engine* setting names, not this
//! crate's names. Two things therefore have to happen before any of it can be
//! applied:
//!
//! 1. Rename. Orca spells a large part of the dialect differently
//!    (`wall_loops` for `perimeters`, `sparse_infill_density` for
//!    `fill_density`). Those pairs are next-layer's own table, proven by its
//!    `--dump-foreign-settings` gate, and are copied here by
//!    `tools/gen-dialect-table.py`.
//! 2. Classify. A name this crate understands is applied; a name that is part
//!    of the dialect but not yet implemented is *recognised and recorded*; a
//!    name that is neither is reported as unknown. Nothing is ever silently
//!    dropped, and a preset is never rejected outright for carrying a setting
//!    this engine does not have — that is the difference between an engine that
//!    accepts the dialect and one that only accepts itself.

use std::collections::BTreeMap;
use std::path::Path;

use crate::dialect_table::{APPLIED, ENGINE_NAMES, NATIVE, ORCA_ONLY, PARTIAL, RENAMES};
use crate::error::ConfigError;
use crate::PrinterConfig;

/// How one key of a preset was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Intake {
    /// Understood and applied: the value reached the engine's configuration.
    Applied,
    /// Understood, applied, and this crate's semantics differ: the value
    /// reached the engine, but not with the meaning the authoring slicer gave
    /// it. The report names the key.
    Partial,
    /// A dialect name this engine does not implement yet: either an engine
    /// setting this crate lacks, or a name only Orca carries. Recognised,
    /// recorded, and carried no further.
    Recognised,
    /// A dialect name with a value this crate rejects (an enum spelling it does
    /// not know, a percentage where it expects a fraction).
    Unconverted,
    /// Not a dialect name at all.
    Unknown,
}

/// The result of feeding a preset to the engine.
#[derive(Debug, Clone, Default)]
pub struct IntakeReport {
    entries: BTreeMap<String, Intake>,
    warnings: Vec<String>,
}

impl IntakeReport {
    fn record(&mut self, key: &str, intake: Intake) {
        self.entries.insert(key.to_owned(), intake);
    }

    /// Every key and how it was handled, in name order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, Intake)> {
        self.entries
            .iter()
            .map(|(key, intake)| (key.as_str(), *intake))
    }

    /// Count of keys handled each way.
    pub fn count(&self, intake: Intake) -> usize {
        self.entries
            .values()
            .filter(|seen| **seen == intake)
            .count()
    }

    /// Total keys in the preset.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the preset carried no keys at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// True when every key was applied: no partial, recognised, unconverted or
    /// unknown name.
    pub fn is_complete(&self) -> bool {
        self.count(Intake::Applied) == self.entries.len()
    }

    /// Percent of keys applied, rounded down.
    pub fn coverage(&self) -> usize {
        if self.entries.is_empty() {
            return 100;
        }
        self.count(Intake::Applied) * 100 / self.entries.len()
    }

    /// Keys handled some other way than applied, with how.
    pub fn gaps(&self) -> Vec<(&str, Intake)> {
        self.entries()
            .filter(|(_, intake)| *intake != Intake::Applied)
            .collect()
    }

    /// Problems with the applied values that the preset's own dialect would
    /// allow but this engine will not print: reported, not enforced, because
    /// refusing the preset teaches the user nothing.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// One line per intake class, for a terminal.
    pub fn summary(&self) -> String {
        format!(
            "{} keys: {} applied, {} partial, {} recognised, {} unconverted, {} unknown ({}% applied)",
            self.len(),
            self.count(Intake::Applied),
            self.count(Intake::Partial),
            self.count(Intake::Recognised),
            self.count(Intake::Unconverted),
            self.count(Intake::Unknown),
            self.coverage(),
        )
    }
}

/// A parsed preset before it is applied: the raw key/value pairs plus the parent
/// it inherits from, if the file names one.
#[derive(Debug, Clone, Default)]
pub struct Preset {
    name: String,
    inherits: Option<String>,
    entries: Vec<(String, String)>,
}

impl Preset {
    /// Parses a `key = value` preset.
    ///
    /// Accepts what PrusaSlicer and OrcaSlicer write: `#` comments, `key=value`
    /// without spaces, quoted values with `\n` escapes, and `key = <<TAG`
    /// heredoc blocks. Unparseable *structure* is an error; an unrecognised
    /// *name* is not — that is the intake report's job.
    pub fn parse(text: &str) -> Result<Preset, ConfigError> {
        if text.trim_start().starts_with('{') {
            let pairs = flat_json(text).ok_or_else(|| ConfigError::BadValue {
                key: "preset",
                why: "a JSON object of settings, one level deep".to_owned(),
            })?;
            return Ok(Preset::from_pairs(pairs));
        }
        let mut preset = Preset::default();
        let mut lines = text.lines().enumerate();
        while let Some((line_number, raw)) = lines.next() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                return Err(ConfigError::SectionedPreset {
                    line: line_number + 1,
                    section: line.to_owned(),
                });
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(ConfigError::MissingEquals {
                    line: line_number + 1,
                });
            };
            let key = key.trim().to_owned();
            let value = value.trim();
            if let Some(tag) = value.strip_prefix("<<") {
                let tag = tag.trim().to_owned();
                let mut body: Vec<&str> = Vec::new();
                let mut closed = false;
                for (_, next) in lines.by_ref() {
                    if next.trim() == tag {
                        closed = true;
                        break;
                    }
                    body.push(next);
                }
                if !closed {
                    return Err(ConfigError::UnclosedBlock {
                        line: line_number + 1,
                        key,
                        tag,
                    });
                }
                preset.entries.push((key, body.join("\n")));
                continue;
            }
            if key == "inherits" {
                preset.inherits = Some(unquote(value));
                continue;
            }
            if key == "name" {
                preset.name = unquote(value);
                continue;
            }
            preset.entries.push((key, unquote(value)));
        }
        Ok(preset)
    }

    /// Builds a preset from pairs that were read out of something that is not a
    /// preset file, such as a 3MF project's settings part.
    pub fn from_pairs(entries: Vec<(String, String)>) -> Preset {
        let mut preset = Preset::default();
        for (key, value) in entries {
            match key.as_str() {
                "inherits" => preset.inherits = Some(unquote(&value)),
                "name" => preset.name = unquote(&value),
                _ => preset.entries.push((key, unquote(&value))),
            }
        }
        preset
    }

    /// Reads a preset and resolves its `inherits` chain inside the same
    /// directory, parent first.
    pub fn load(path: &Path) -> Result<Preset, ConfigError> {
        let mut chain: Vec<Preset> = Vec::new();
        let mut current = Some(path.to_path_buf());
        let mut seen: Vec<String> = Vec::new();
        while let Some(file) = current {
            let text = std::fs::read_to_string(&file).map_err(|error| ConfigError::Read {
                path: file.display().to_string(),
                message: error.to_string(),
            })?;
            let preset = Preset::parse(&text)?;
            seen.push(file.display().to_string());
            current = preset.inherits.as_ref().and_then(|parent| {
                let sibling = file.with_file_name(format!("{parent}.ini"));
                if sibling.is_file() {
                    Some(sibling)
                } else {
                    let sibling = file.with_file_name(format!("{parent}.cfg"));
                    sibling.is_file().then_some(sibling)
                }
            });
            if let Some(next) = &current {
                if seen.iter().any(|path| path == &next.display().to_string()) {
                    return Err(ConfigError::RecursivePreset { chain: seen });
                }
            }
            chain.push(preset);
        }
        let mut resolved = Preset::default();
        for preset in chain.into_iter().rev() {
            resolved.inherits = None;
            if !preset.name.is_empty() {
                resolved.name = preset.name;
            }
            resolved.entries.extend(preset.entries);
        }
        Ok(resolved)
    }

    /// This preset on top of a parent: the child's values win, key by key.
    pub fn with_base(mut self, base: &Preset) -> Preset {
        let mut merged: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in base.entries.iter().chain(self.entries.iter()) {
            merged.insert(key.clone(), value.clone());
        }
        if self.name.is_empty() {
            self.name = base.name.clone();
        }
        self.entries = merged.into_iter().collect();
        self.inherits = None;
        self
    }

    /// The preset's own name, from a `name` line or its `inherits`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The parent preset name the file declares, if any.
    pub fn inherits(&self) -> Option<&str> {
        self.inherits.as_deref()
    }

    /// Raw keys in file order.
    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }
}

impl PrinterConfig {
    /// Reads a PrusaSlicer/Orca preset, applying every name this crate
    /// understands and reporting the rest.
    pub fn load_preset(path: &Path) -> Result<(PrinterConfig, IntakeReport), ConfigError> {
        let preset = Preset::load(path)?;
        Ok(PrinterConfig::from_preset(&preset))
    }

    /// Applies a preset, returning the configuration and how every key was
    /// handled. A name this engine does not implement is never an error.
    pub fn from_preset(preset: &Preset) -> (PrinterConfig, IntakeReport) {
        let mut config = PrinterConfig::default();
        let mut report = IntakeReport::default();
        for (key, value) in preset.entries() {
            if key.starts_with('$') {
                continue;
            }
            let (name, intake) = resolve(key);
            match intake {
                Intake::Applied | Intake::Partial => {
                    let scaled = convert_value(&name, value);
                    if name == "start_gcode" || name == "end_gcode" {
                        if name == "start_gcode" {
                            config.start_gcode = scaled;
                        } else {
                            config.end_gcode = scaled;
                        }
                        report.record(key, intake);
                        continue;
                    }
                    match config.assign(0, &name, &scaled) {
                        Ok(()) if intake == Intake::Applied => {
                            report.record(key, Intake::Applied);
                        }
                        Ok(()) => report.record(key, Intake::Partial),
                        Err(_) => report.record(key, Intake::Unconverted),
                    }
                }
                Intake::Recognised => report.record(key, Intake::Recognised),
                Intake::Unconverted | Intake::Unknown => report.record(key, Intake::Unknown),
            }
        }
        if !preset.name().is_empty() {
            config.name = preset.name().to_owned();
        }
        config.adopt_bed_shape();
        config.adopt_speed_ceiling();
        fill_placeholders(&mut config, preset, &mut report);
        if let Err(problem) = config.resolve_widths(preset.entries()) {
            report.warnings.push(problem.to_string());
        }
        if let Err(problem) = config.validate() {
            report.warnings.push(problem.to_string());
        }
        (config, report)
    }
}

/// Tokens that the reference dialects write into G-code and that mean, here, one of
/// this engine's own per-layer placeholders.
const DYNAMIC_PLACEHOLDERS: &[(&str, &str)] = &[
    ("layer_z", "{layer_z}"),
    ("layer_num", "{layer_num}"),
    ("total_layer_count", "{total_layers}"),
    ("current_extruder", "0"),
];

/// Substitutes the reference dialects' placeholders in the G-code a preset carries.
///
/// A machine preset's start G-code is a template, and the two dialects template it
/// differently: `[name]` is a setting, `{name}` is a variable, and an index after
/// the name (`[first_layer_print_min[0]]`, `{fan[current_extruder]}`) selects an
/// element. This engine's own G-code template language is `{name}` with a small
/// vocabulary, so a preset's text is translated into it:
///
/// - a placeholder whose meaning is per-layer here is rewritten to this engine's
///   spelling, because its value is only known once a layer is being written;
/// - a placeholder naming a setting is replaced with that setting's value, taken
///   from the preset itself first (it is authoritative: the user's own profile) and
///   from the engine's configuration second;
/// - anything else is left as literal text and reported, because a machine's macro
///   silently receiving the word `[foo]` instead of a number is how a print starts
///   wrong.
///
/// Values are flattened the way the dialect stores them: an array is comma-separated
/// and a per-tool setting takes its first entry, because this engine prints with one
/// extruder.
fn fill_placeholders(config: &mut PrinterConfig, preset: &Preset, report: &mut IntakeReport) {
    let pairs: Vec<(String, String)> = preset.entries().to_vec();
    let known: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| (key.clone(), first_value(value)))
        .collect();
    let keys = [
        "start_gcode",
        "end_gcode",
        "before_layer_gcode",
        "layer_gcode",
    ];
    let mut unresolved: Vec<String> = Vec::new();
    for key in keys {
        let Some((written, text)) = preset
            .entries()
            .iter()
            .find(|(name, _)| resolve(name).0 == key)
        else {
            continue;
        };
        let lookup = |name: &str, index: Option<usize>| -> Option<String> {
            pairs
                .iter()
                .find(|(candidate, _)| candidate == name)
                .and_then(|(_, value)| match index {
                    Some(index) => value
                        .split(',')
                        .nth(index)
                        .map(|part| part.trim().trim_matches('"').to_owned()),
                    None => Some(first_value(value)),
                })
                .or_else(|| intrinsic(config, name))
        };
        let (conditioned, expressions) = crate::expression::expand(text, &lookup);
        unresolved.extend(expressions);
        let mut filled = String::with_capacity(conditioned.len());
        let mut rest = conditioned.as_str();
        while let Some(at) = rest.find(['[', '{']) {
            filled.push_str(&rest[..at]);
            let open = rest.as_bytes()[at] as char;
            let close = if open == '[' { ']' } else { '}' };
            let after = &rest[at + 1..];
            let Some(end) = after.find(close) else {
                filled.push_str(&rest[at..]);
                rest = "";
                break;
            };
            let token = &after[..end];
            rest = &after[end + 1..];
            let name = strip_index(token);
            if let Some((_, ours)) = DYNAMIC_PLACEHOLDERS
                .iter()
                .find(|(theirs, _)| *theirs == name)
            {
                filled.push_str(ours);
                continue;
            }
            let value = known
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .or_else(|| intrinsic(config, name));
            match value {
                Some(value) => filled.push_str(&value),
                None => {
                    if !unresolved.iter().any(|seen| seen == name) {
                        unresolved.push(name.to_owned());
                    }
                    filled.push(open);
                    filled.push_str(token);
                    filled.push(close);
                }
            }
        }
        filled.push_str(rest);
        assign_gcode(config, key, filled);
        if !unresolved.is_empty() {
            report.record(written, Intake::Partial);
        }
    }
    if unresolved.is_empty() {
        return;
    }
    let shown = unresolved
        .iter()
        .take(4)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let more = match unresolved.len() {
        0..=4 => String::new(),
        count => format!(" and {} more", count - 4),
    };
    report.warnings.push(format!(
        "machine G-code uses {} placeholder(s) this engine does not know: {shown}{more}; \
         they would reach the printer as literal text",
        unresolved.len()
    ));
}

/// A token's setting name, without the element index the dialect may append.
fn strip_index(token: &str) -> &str {
    match token.find('[') {
        Some(at) => &token[..at],
        None => token,
    }
}

/// The first element of a dialect value, which stores lists as comma-separated text.
fn first_value(value: &str) -> String {
    value
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .to_owned()
}

/// A placeholder's value when the preset does not carry it but the engine does.
fn intrinsic(config: &PrinterConfig, name: &str) -> Option<String> {
    let number = |value: f64| Some(format!("{value}"));
    match name {
        "first_layer_temperature" => number(config.first_nozzle_temp),
        "temperature" | "nozzle_temperature" => number(config.nozzle_temp),
        "first_layer_bed_temperature" | "bed_temperature_initial_layer_single" => {
            number(if config.first_layer_bed_temp > 0.0 {
                config.first_layer_bed_temp
            } else {
                config.bed_temp
            })
        }
        "bed_temperature" => number(config.bed_temp),
        "nozzle_diameter" => number(config.nozzle_diameter),
        "num_extruders" => Some("1".to_owned()),
        "bed_shape" => Some(config.bed_shape_text()),
        "max_layer_z" | "max_print_height" => number(config.max_z),
        _ => None,
    }
}

fn assign_gcode(config: &mut PrinterConfig, key: &str, text: String) {
    match key {
        "start_gcode" => config.start_gcode = text,
        "end_gcode" => config.end_gcode = text,
        "before_layer_gcode" => config.before_layer_gcode = text,
        "layer_gcode" => config.layer_gcode = text,
        _ => {}
    }
}

/// Maps one preset key to the name this crate uses, and says how it is known.
///
/// The name returned is the one handed to `PrinterConfig::assign`, so it must be
/// a key the parser accepts: a native key stays itself, and a renamed key takes
/// its native spelling. The reported intake is decided separately, because a key
/// the table marks `partial` is still read — only with this engine's meaning, not
/// the authoring slicer's.
fn resolve(key: &str) -> (String, Intake) {
    let engine = RENAMES
        .iter()
        .find(|(source, _)| *source == key)
        .map(|(_, target)| *target)
        .unwrap_or(key);
    let (name, known) = if NATIVE.contains(&key) {
        (key.to_owned(), Intake::Applied)
    } else if NATIVE.contains(&engine) {
        (engine.to_owned(), Intake::Applied)
    } else if let Some((_, native)) = APPLIED.iter().find(|(name, _)| *name == engine) {
        ((*native).to_owned(), Intake::Applied)
    } else if let Some((_, native)) = PARTIAL.iter().find(|(name, _)| *name == engine) {
        ((*native).to_owned(), Intake::Partial)
    } else if ENGINE_NAMES.contains(&engine) {
        (engine.to_owned(), Intake::Recognised)
    } else if ORCA_ONLY.contains(&key) {
        (key.to_owned(), Intake::Recognised)
    } else {
        (key.to_owned(), Intake::Unknown)
    };
    let partial = PARTIAL.iter().any(|(name, _)| *name == engine);
    let intake = match (known, partial) {
        (Intake::Applied | Intake::Partial, true) => Intake::Partial,
        (known, _) => known,
    };
    (name, intake)
}

/// Converts the value spellings the two dialects disagree about, where the
/// disagreement is mechanical: a percentage into a fraction.
pub(crate) fn convert_value(key: &str, value: &str) -> String {
    const FRACTIONAL: &[&str] = &[
        "infill_density",
        "support_density",
        "support_interface_density",
        "ironing_flow",
    ];
    if !FRACTIONAL.contains(&key) {
        return value.to_owned();
    }
    match value.trim().strip_suffix('%') {
        Some(number) => match number.trim().parse::<f64>() {
            Ok(percent) => format!("{}", percent / 100.0),
            Err(_) => value.to_owned(),
        },
        None => value.to_owned(),
    }
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    // A comma list quotes each entry, so the value starts and ends with a quote
    // without being one quoted string. Stripping both ends there would leave the
    // first and last entries unterminated and split the list in the wrong place.
    let single_quoted = trimmed.len() >= 2
        && trimmed.starts_with('"')
        && trimmed.ends_with('"')
        && !trimmed[1..trimmed.len() - 1].contains('"');
    let inner = if single_quoted {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    inner.replace("\\n", "\n").replace("\\\"", "\"")
}

/// A 3MF project's own settings, read from the metadata parts its container
/// carries next to the geometry.
///
/// A `.3mf` written by a slicer is a project, not a model: the teeth of a job
/// live in a sibling part under `Metadata/`. Reading them is what makes
/// `hfw project.3mf` slice the way the project was saved instead of the way
/// this engine's defaults say. Nothing here is applied by guessing: the part is
/// turned into the same [`Preset`] a `--preset` file produces, so it goes
/// through the identical rename-and-classify path and produces the identical
/// intake report.
#[derive(Debug, Clone, Default)]
pub struct ProjectSettings {
    part: String,
    preset: Preset,
    object_overrides: usize,
}

impl ProjectSettings {
    /// Reads the project settings out of `Metadata/` parts, if any carry them.
    ///
    /// PrusaSlicer and OrcaSlicer write `Metadata/Slic3r_PE.config` — the same
    /// `key = value` preset a `.ini` export holds. Bambu Studio and newer Orca
    /// write `Metadata/project_settings.config`, a flat JSON object of the same
    /// names. Both are understood; the INI part wins when a file has both,
    /// because it is the one whose format is documented by its own writer.
    pub fn from_metadata(parts: &[(String, Vec<u8>)]) -> Option<ProjectSettings> {
        let ini = parts
            .iter()
            .find(|(name, _)| name == "Metadata/Slic3r_PE.config")
            .map(|(name, bytes)| (name.clone(), String::from_utf8_lossy(bytes).into_owned()));
        let json = parts
            .iter()
            .find(|(name, _)| name == "Metadata/project_settings.config")
            .map(|(name, bytes)| (name.clone(), String::from_utf8_lossy(bytes).into_owned()));
        let object_overrides = parts
            .iter()
            .find(|(name, _)| name == "Metadata/Slic3r_PE_model.config")
            .map(|(_, bytes)| String::from_utf8_lossy(bytes).matches("<object ").count())
            .unwrap_or(0);
        let (part, text, is_json) = match (ini, json) {
            (Some((part, text)), _) => (part, text, false),
            (None, Some((part, text))) => (part, text, true),
            (None, None) => return None,
        };
        let preset = if is_json {
            Preset::from_pairs(flat_json(&text)?)
        } else {
            Preset::parse(&text).ok()?
        };
        Some(ProjectSettings {
            part,
            preset,
            object_overrides,
        })
    }

    /// The part the settings were read from.
    pub fn part(&self) -> &str {
        &self.part
    }

    /// The settings as a preset, ready for the normal intake path.
    pub fn preset(&self) -> &Preset {
        &self.preset
    }

    /// How many objects carry their own overrides in the file.
    ///
    /// Per-object settings are a separate feature; the count is reported so a
    /// project that has them is never silently sliced with them ignored.
    pub fn object_overrides(&self) -> usize {
        self.object_overrides
    }
}

/// Reads a JSON object of settings into pairs, or `None` when the text is not one.
///
/// Two shapes are read. A preset is one level of `"name": value`, where a value is
/// a scalar or an array of scalars; arrays are joined with commas, which is how
/// every slicer writes delimiter-separated lists. The reference slicers' own
/// `--configuration-save` is the other: one level of objects named
/// `printer_settings`, `print_settings` and `filament_settings`, each of which is
/// exactly the first shape. A nested object is therefore read as more settings
/// rather than skipped, which is what lets the same reader take a preset, a
/// project's settings part, and a full configuration dump.
pub(crate) fn flat_json(text: &str) -> Option<Vec<(String, String)>> {
    let mut rest = text.trim_start();
    rest = rest.strip_prefix('{')?;
    let mut pairs = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.starts_with('}') {
            return Some(pairs);
        }
        let (key, after_key) = json_string(rest.trim_start())?;
        rest = after_key.trim_start().strip_prefix(':')?;
        rest = rest.trim_start();
        if rest.starts_with('{') {
            let after = skip_balanced(rest, '{', '}')?;
            if let Some(nested) = flat_json(rest) {
                // A setting's value is itself an object only when the writer had
                // more to say than one number: `{"is_percent": true, "value": 85}`
                // is 85%, and reading it as two settings would both lose the
                // value and invent keys named `value` and `is_percent`.
                if let Some(value) = percent_setting(&nested) {
                    pairs.push((key, value));
                } else {
                    // An empty value in a configuration dump means "inherit", not
                    // "clear": every section of the dump carries the full key list,
                    // so a later section's blank start G-code would otherwise erase
                    // the printer's own.
                    pairs.extend(
                        nested
                            .into_iter()
                            .filter(|(_, value)| !value.trim().is_empty()),
                    );
                }
            }
            rest = after;
        } else {
            let (value, after) = json_value(rest)?;
            rest = after;
            if let Some(value) = value {
                pairs.push((key, value));
            }
        }
        rest = rest.trim_start();
        if let Some(after_comma) = rest.strip_prefix(',') {
            rest = after_comma;
            continue;
        }
        if rest.starts_with('}') {
            return Some(pairs);
        }
        return None;
    }
}

/// A value object's own settings when the object is the writer's spelling of a
/// percentage, `{"is_percent": true, "value": 85}`.
fn percent_setting(nested: &[(String, String)]) -> Option<String> {
    let value = |name: &str| {
        nested
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.trim())
    };
    if nested.len() != 2 || value("is_percent").is_none() {
        return None;
    }
    let number = value("value")?;
    let percent = matches!(value("is_percent")?, "true" | "True" | "1");
    Some(if percent {
        format!("{number}%")
    } else {
        number.to_owned()
    })
}

fn json_string(text: &str) -> Option<(String, &str)> {
    let rest = text.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((out, &rest[at + 1..])),
            '\\' => {
                let (_, escaped) = chars.next()?;
                match escaped {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'u' => {
                        let mut code = String::new();
                        for _ in 0..4 {
                            code.push(chars.next()?.1);
                        }
                        let value = u32::from_str_radix(&code, 16).ok()?;
                        out.push(char::from_u32(value)?);
                    }
                    other => out.push(other),
                }
            }
            other => out.push(other),
        }
    }
    None
}

fn json_value(text: &str) -> Option<(Option<String>, &str)> {
    if text.starts_with('"') {
        let (value, rest) = json_string(text)?;
        return Some((Some(value), rest));
    }
    if text.starts_with('[') {
        let rest = text.strip_prefix('[')?;
        let mut items = Vec::new();
        let mut rest = rest;
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix(']') {
                return Some((Some(items.join(",")), after));
            }
            let (value, after) = json_value(rest)?;
            rest = after;
            if let Some(value) = value {
                items.push(value);
            }
            rest = rest.trim_start();
            if let Some(after_comma) = rest.strip_prefix(',') {
                rest = after_comma;
            }
        }
    }
    if text.starts_with("null") {
        return Some((None, text.strip_prefix("null")?));
    }
    let end = text
        .find(|c: char| c == ',' || c == '}' || c == ']' || c.is_whitespace())
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    Some((Some(text[..end].to_owned()), &text[end..]))
}

fn skip_balanced(text: &str, open: char, close: char) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[at + 1..]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Settings a slicer project overrides on one object of its plate.
///
/// PrusaSlicer and OrcaSlicer keep these in `Metadata/Slic3r_PE_model.config`,
/// one `<object id="N">` per model object with `<metadata type="object"
/// key="…" value="…"/>` children. The values are the same names a preset uses,
/// so they go through the same rename table; what differs is that they apply to
/// one object rather than the whole plate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectOverride {
    /// Object id as the model part numbers it.
    pub id: u32,
    /// Object name, when the project stores one.
    pub name: Option<String>,
    /// Raw key/value overrides in file order.
    pub pairs: Vec<(String, String)>,
}

/// Reads per-object overrides out of a project's metadata parts.
///
/// Volume- and layer-range-level metadata is deliberately not read: those need
/// modifier volumes this engine does not have, and returning them as if they
/// applied to the whole object would print something the file did not ask for.
pub fn object_overrides(parts: &[(String, Vec<u8>)]) -> Vec<ObjectOverride> {
    for (name, bytes) in parts {
        if name == "Metadata/Slic3r_PE_model.config" {
            return parse_model_config(&String::from_utf8_lossy(bytes));
        }
    }
    Vec::new()
}

fn parse_model_config(text: &str) -> Vec<ObjectOverride> {
    let mut out: Vec<ObjectOverride> = Vec::new();
    for tag in xml_tags(text) {
        let name = tag.split_whitespace().next().unwrap_or("");
        match name {
            "object" => {
                let id = xml_attribute(tag, "id")
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .unwrap_or(0);
                out.push(ObjectOverride {
                    id,
                    name: None,
                    pairs: Vec::new(),
                });
            }
            "metadata" => {
                let Some(current) = out.last_mut() else {
                    continue;
                };
                if xml_attribute(tag, "type").as_deref() != Some("object") {
                    continue;
                }
                let (Some(key), Some(value)) =
                    (xml_attribute(tag, "key"), xml_attribute(tag, "value"))
                else {
                    continue;
                };
                let key = key.trim().to_owned();
                let value = unescape_xml(value.trim());
                if key == "name" {
                    current.name = Some(value);
                } else {
                    current.pairs.push((key, value));
                }
            }
            _ => {}
        }
    }
    out.retain(|object| !object.pairs.is_empty() || object.name.is_some());
    out
}

/// Start tags of an XML document, without their angle brackets.
fn xml_tags(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let after = &rest[start..];
        if after.starts_with("<?") || after.starts_with("<!") {
            match after.find('>') {
                Some(end) => {
                    rest = &after[end + 1..];
                    continue;
                }
                None => break,
            }
        }
        match after.find('>') {
            Some(end) => {
                let tag = &after[1..end];
                if !tag.starts_with('/') {
                    out.push(tag.trim_end_matches('/').trim());
                }
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    out
}

fn xml_attribute(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    loop {
        let trimmed = rest.trim_start();
        let key_end = trimmed
            .find(|character: char| character.is_whitespace() || character == '=')
            .unwrap_or(trimmed.len());
        let key = &trimmed[..key_end];
        let after_key = trimmed[key_end..].trim_start();
        if let Some(after_equals) = after_key.strip_prefix('=') {
            let quoted = after_equals.trim_start();
            let quote = quoted.chars().next()?;
            if quote != '"' && quote != '\'' {
                return None;
            }
            let inner = &quoted[1..];
            let end = inner.find(quote)?;
            if key == name {
                return Some(inner[..end].to_owned());
            }
            rest = &inner[end + 1..];
        } else if after_key.is_empty() {
            return None;
        } else {
            rest = after_key;
        }
    }
}

fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLATE_MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1">
    <metadata type="object" key="name" value="cube"/>
  </object>
  <object id="2">
    <metadata type="object" key="name" value="dense cube"/>
    <metadata type="object" key="sparse_infill_density" value="60%"/>
    <metadata type="object" key="wall_loops" value="5"/>
    <volume firstid="8" lastid="15">
      <metadata type="volume" key="sparse_infill_density" value="5%"/>
    </volume>
  </object>
</config>
"#;

    #[test]
    fn a_presets_bed_outline_reaches_the_bed_size_placement_uses() {
        let preset = Preset::parse("bed_shape = 0x0,350x0,350x350,0x350\nnozzle_diameter = 0.4\n")
            .expect("preset parses");
        let (config, _report) = PrinterConfig::from_preset(&preset);
        assert!((config.bed_x - 350.0).abs() < 1e-9);
        assert!((config.bed_y - 350.0).abs() < 1e-9);
    }

    #[test]
    fn per_object_overrides_are_read_and_volume_metadata_is_not() {
        let parts = vec![(
            "Metadata/Slic3r_PE_model.config".to_owned(),
            PLATE_MODEL.as_bytes().to_vec(),
        )];
        let objects = object_overrides(&parts);
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].id, 1);
        assert_eq!(objects[0].name.as_deref(), Some("cube"));
        assert!(
            objects[0].pairs.is_empty(),
            "a bare name is not an override"
        );
        assert_eq!(objects[1].id, 2);
        assert_eq!(objects[1].name.as_deref(), Some("dense cube"));
        assert_eq!(
            objects[1].pairs,
            vec![
                ("sparse_infill_density".to_owned(), "60%".to_owned()),
                ("wall_loops".to_owned(), "5".to_owned()),
            ],
            "volume-scoped metadata must not become an object override"
        );
    }

    #[test]
    fn a_project_without_object_metadata_has_no_overrides() {
        let parts = vec![(
            "Metadata/Slic3r_PE.config".to_owned(),
            b"wall_loops = 4\n".to_vec(),
        )];
        assert!(object_overrides(&parts).is_empty());
    }

    const ORCA: &str = "\
# exported by OrcaSlicer
wall_loops = 4
layer_height = 0.24
first_layer_height = 0.3
sparse_infill_density = 15%
sparse_infill_pattern = gyroid
nozzle_temperature = 215
hot_plate_temp = 60
enable_ams = 1
accel_to_decel_enable = 1
";

    #[test]
    fn every_table_row_resolves_to_a_key_this_parser_accepts() {
        // A row whose resolved name this parser does not accept claims support
        // that does not exist: `resolve` hands that name to `assign`, which
        // rejects it, and the preset key is recorded as unconverted while the
        // ledger calls the setting applied.
        for (engine, native) in APPLIED.iter().chain(PARTIAL.iter()) {
            if matches!(*engine, "start_gcode" | "end_gcode") {
                continue;
            }
            let (name, intake) = resolve(engine);
            assert_ne!(
                intake,
                Intake::Recognised,
                "{engine}: row is not applied or partial"
            );
            let mut config = PrinterConfig::default();
            if let Err(ConfigError::UnknownKey { .. }) = config.assign(0, &name, "1") {
                panic!("row ({engine}, {native}) resolves to '{name}', which this parser does not accept");
            }
        }
    }

    #[test]
    fn renames_orca_spellings() {
        let preset = Preset::parse(ORCA).expect("parses");
        let (config, report) = PrinterConfig::from_preset(&preset);
        assert_eq!(config.walls, 4);
        assert_eq!(config.layer_height, 0.24);
        assert_eq!(config.first_layer_height, 0.3);
        assert!((config.infill_density - 0.15).abs() < 1e-9);
        assert_eq!(config.infill_pattern, "gyroid");
        assert_eq!(config.nozzle_temp, 215.0);
        assert!(report.count(Intake::Applied) >= 5);
        assert!(
            report
                .entries()
                .any(|(key, intake)| key == "sparse_infill_pattern" && intake == Intake::Partial),
            "gyroid has no geometry here, so the pattern is carried but not printed"
        );
    }

    #[test]
    fn accepts_engine_names_without_a_rename() {
        let preset = Preset::parse("perimeters = 5\nlayer_height = 0.2\n").expect("parses");
        let (config, _report) = PrinterConfig::from_preset(&preset);
        assert_eq!(config.walls, 5);
        assert_eq!(config.layer_height, 0.2);
    }

    #[test]
    fn keeps_native_profiles_working() {
        let preset = Preset::parse("walls = 3\ninfill_density = 0.25\n").expect("parses");
        let (config, report) = PrinterConfig::from_preset(&preset);
        assert_eq!(config.walls, 3);
        assert_eq!(config.infill_density, 0.25);
        assert!(report.is_complete());
    }

    #[test]
    fn unknown_names_are_recorded_not_fatal() {
        let preset = Preset::parse("enable_ams = 1\nsome_bambu_thing = 7\n").expect("parses");
        let (_config, report) = PrinterConfig::from_preset(&preset);
        assert_eq!(report.count(Intake::Unknown), 2);
        assert_eq!(report.coverage(), 0);
        assert_eq!(report.len(), 2);
    }

    #[test]
    fn dialect_names_the_engine_lacks_are_recognised() {
        let preset = Preset::parse("wipe_tower_width = 60\n").expect("parses");
        let (_config, report) = PrinterConfig::from_preset(&preset);
        assert_eq!(report.count(Intake::Recognised), 1);
        assert_eq!(report.gaps()[0].0, "wipe_tower_width");
    }

    #[test]
    fn a_renamed_name_this_engine_defines_itself_is_applied() {
        let preset = Preset::parse("support_interface_spacing = 0.5\n").expect("parses");
        let (config, report) = PrinterConfig::from_preset(&preset);
        assert!((config.support_interface_spacing - 0.5).abs() < 1e-9);
        assert_eq!(report.count(Intake::Applied), 1);
    }

    #[test]
    fn an_unfamiliar_enum_value_is_unconverted_not_a_failure() {
        let preset = Preset::parse("support_style = organic\n").expect("parses");
        let (config, report) = PrinterConfig::from_preset(&preset);
        assert_eq!(config.support_style, PrinterConfig::default().support_style);
        assert!(report
            .gaps()
            .iter()
            .any(|(_, intake)| *intake == Intake::Unconverted));
    }

    #[test]
    fn child_values_win_and_inherits_is_not_a_setting() {
        let parent =
            Preset::parse("name = base\nwall_loops = 3\nlayer_height = 0.2\n").expect("parses");
        let child = Preset::parse("inherits = base\nwall_loops = 5\n").expect("parses");
        assert_eq!(child.inherits(), Some("base"));
        let merged = child.with_base(&parent);
        let (config, _report) = PrinterConfig::from_preset(&merged);
        assert_eq!(config.walls, 5);
        assert_eq!(config.layer_height, 0.2);
    }

    #[test]
    fn quoted_values_with_escapes_are_unescaped() {
        let preset = Preset::parse("start_gcode = \"G28\\nG1 Z5\"\n").expect("parses");
        let (config, _report) = PrinterConfig::from_preset(&preset);
        assert_eq!(config.start_gcode, "G28\nG1 Z5");
    }

    #[test]
    fn heredoc_blocks_are_read() {
        let text = "start_gcode = <<END\nG28\nG1 Z5\nEND\n";
        let preset = Preset::parse(text).expect("parses");
        let (config, _report) = PrinterConfig::from_preset(&preset);
        assert_eq!(config.start_gcode, "G28\nG1 Z5");
    }

    #[test]
    fn a_percentage_reaches_a_fractional_setting() {
        let preset = Preset::parse("fill_density = 20%\n").expect("parses");
        let (config, report) = PrinterConfig::from_preset(&preset);
        assert!((config.infill_density - 0.2).abs() < 1e-9);
        assert_eq!(report.count(Intake::Applied), 1);
    }

    #[test]
    fn a_vendor_bundle_of_several_presets_is_named_rather_than_misread() {
        let text = "[print:0.20mm Quality]\nlayer_height = 0.2\n";
        let error = Preset::parse(text).expect_err("a bundle is not a preset");
        assert!(error.to_string().contains("[print:0.20mm Quality]"));
    }

    #[test]
    fn a_value_the_engine_will_not_print_is_a_warning_not_a_refusal() {
        let preset = Preset::parse("layer_height = 0.5\nnozzle_diameter = 0.4\n").expect("parses");
        let (config, report) = PrinterConfig::from_preset(&preset);
        assert_eq!(config.layer_height, 0.5);
        assert_eq!(report.warnings().len(), 1);
        assert_eq!(report.count(Intake::Applied), 2);
    }

    #[test]
    fn the_report_is_stable_across_runs() {
        let preset = Preset::parse(ORCA).expect("parses");
        let first = PrinterConfig::from_preset(&preset).1.summary();
        let second = PrinterConfig::from_preset(&preset).1.summary();
        assert_eq!(first, second);
    }
}
