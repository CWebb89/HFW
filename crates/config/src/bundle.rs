//! Resolving a slicer's preset bundle into one job's configuration.
//!
//! A bundle is what the reference slicers ship and what a user's own profile
//! collection looks like on disk: many presets, each of one of three kinds
//! (machine, process, filament), each possibly inheriting from another by name.
//! Slicing one job means picking one of each and layering them, which is the step
//! that sits between "the users' presets exist" and "the engine can be handed a
//! job", and it is not the same problem as reading a single preset file.
//!
//! Two on-disk shapes are read:
//!
//! - A directory of preset files, which is how OrcaSlicer stores a bundle
//!   (`machine/`, `process/`, `filament/` of `.json`), and how either slicer
//!   writes a single exported preset.
//! - One `.ini` or `.config` holding many presets as `[kind:name]` sections,
//!   which is how PrusaSlicer ships a vendor profile.
//!
//! A parent that is not in the bundle is *reported*, not fatal: an exported
//! profile routinely inherits from a system preset that only exists inside the
//! slicer's application directory. A name that is not in the bundle at all is
//! fatal, because that is a typo rather than a missing application preset.

use std::path::{Path, PathBuf};

use crate::dialect::{flat_json, Preset};
use crate::{ConfigError, IntakeReport, PrinterConfig};

/// What a preset in a bundle configures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Kind {
    /// The machine: bed, limits, g-code flavour, start/end g-code.
    Machine,
    /// The print settings: layer height, walls, infill, support.
    Process,
    /// One material: temperatures, cooling, flow limits.
    Filament,
}

impl Kind {
    /// The word this kind is called by, as a bundle spells it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Machine => "machine",
            Kind::Process => "process",
            Kind::Filament => "filament",
        }
    }

    /// Reads a kind from the word a preset or a directory uses for it.
    fn parse(word: &str) -> Option<Kind> {
        match word.trim().to_ascii_lowercase().as_str() {
            "machine" | "printer" | "printers" | "machines" => Some(Kind::Machine),
            "process" | "print" | "processes" | "prints" => Some(Kind::Process),
            "filament" | "filaments" | "material" => Some(Kind::Filament),
            _ => None,
        }
    }

    /// Reads a kind from a key that only one kind of preset carries.
    ///
    /// A directory of flat presets has no `type` field, and an exported
    /// PrusaSlicer preset has none either, so the file's own contents decide. The
    /// test is a key no other kind writes: `nozzle_diameter` is a machine,
    /// `filament_type` a material, `layer_height` a process.
    fn sniff(entries: &[(String, String)]) -> Option<Kind> {
        let has = |name: &str| entries.iter().any(|(key, _)| key == name);
        if has("nozzle_diameter") || has("bed_shape") || has("printer_model") {
            return Some(Kind::Machine);
        }
        if has("filament_type") || has("filament_density") || has("filament_settings_id") {
            return Some(Kind::Filament);
        }
        if has("layer_height") || has("print_settings_id") || has("perimeters") {
            return Some(Kind::Process);
        }
        None
    }
}

/// One preset found in the bundle.
#[derive(Clone, Debug)]
struct Entry {
    kind: Kind,
    name: String,
    parents: Vec<String>,
    entries: Vec<(String, String)>,
    /// Vendor directory the preset sits under, when the bundle has one.
    vendor: Option<String>,
}

/// One `[kind:name]` section of a sectioned vendor file: its kind when the header
/// names one, its name, and the settings under it.
type Section = (Option<Kind>, String, Vec<(String, String)>);

/// Every preset under one bundle root, indexed by kind and name.
#[derive(Clone, Debug, Default)]
pub struct Bundle {
    root: PathBuf,
    entries: Vec<Entry>,
    warnings: Vec<String>,
}

/// A bundle preset with its inheritance chain already merged.
#[derive(Clone, Debug)]
pub struct Resolution {
    preset: Preset,
    warnings: Vec<String>,
}

impl Resolution {
    /// The merged preset, parent first, ready for the normal intake path.
    pub fn preset(&self) -> &Preset {
        &self.preset
    }

    /// Parents named by the chain that the bundle does not contain.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl Bundle {
    /// Indexes every preset under `root`.
    ///
    /// Reads a file as a sectioned bundle when its first non-comment line is a
    /// `[section]` header, and as a single preset otherwise. Unknown file types
    /// are skipped: a bundle directory usually holds thumbnails and READMEs too.
    pub fn discover(root: &Path) -> Result<Bundle, ConfigError> {
        let mut bundle = Bundle {
            root: root.to_path_buf(),
            entries: Vec::new(),
            warnings: Vec::new(),
        };
        if !root.is_dir() {
            return Err(ConfigError::Read {
                path: root.display().to_string(),
                message: "not a directory".to_owned(),
            });
        }
        let mut files = Vec::new();
        collect(root, 0, &mut files)?;
        files.sort();
        for file in files {
            let text = match std::fs::read_to_string(&file) {
                Ok(text) => text,
                Err(error) => {
                    bundle
                        .warnings
                        .push(format!("{}: {}", file.display(), error));
                    continue;
                }
            };
            if is_json(&file) {
                bundle.read_json(&file, &text);
            } else if starts_with_section(&text) {
                bundle.read_sections(&file, &text);
            } else {
                bundle.read_single(&file, &text);
            }
        }
        if bundle.entries.is_empty() {
            return Err(ConfigError::BundleEmpty {
                path: root.display().to_string(),
            });
        }
        Ok(bundle)
    }

    /// The bundle's root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Problems met while indexing, which never stop a preset from resolving.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The names of every preset of one kind, sorted.
    pub fn names(&self, kind: Kind) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .map(|entry| entry.name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// How many presets of each kind the bundle holds, as one line.
    pub fn summary(&self) -> String {
        format!(
            "{} machine(s), {} process(es), {} filament(s)",
            self.names(Kind::Machine).len(),
            self.names(Kind::Process).len(),
            self.names(Kind::Filament).len()
        )
    }

    /// Merges one preset's inheritance chain, parents first.
    ///
    /// A parent missing from the bundle is reported in the result's warnings and
    /// skipped; the preset on disk is still usable, because an exported profile
    /// routinely inherits from a system preset that ships inside the slicer.
    pub fn resolve(&self, kind: Kind, name: &str) -> Result<Resolution, ConfigError> {
        self.resolve_with(kind, name, None)
    }

    /// Merges one chain, preferring presets under a named vendor directory.
    ///
    /// The preference is what makes a library of several vendors usable: asked for
    /// "Generic PLA" under a Voron machine, the Voron copy is meant, not whichever
    /// vendor's file sorts first.
    pub fn resolve_with(
        &self,
        kind: Kind,
        name: &str,
        prefer_vendor: Option<&str>,
    ) -> Result<Resolution, ConfigError> {
        let mut chain: Vec<&Entry> = Vec::new();
        let mut visiting: Vec<String> = Vec::new();
        self.walk(kind, name, prefer_vendor, &mut chain, &mut visiting)?;
        let mut merged = Preset::default();
        for entry in chain {
            let own = Preset::from_pairs(entry.entries.clone());
            merged = own.with_base(&merged);
        }
        let mut named = vec![(String::from("name"), name.to_owned())];
        named.extend(merged.entries().iter().cloned());
        Ok(Resolution {
            preset: Preset::from_pairs(named),
            warnings: self.missing_parents(kind, name),
        })
    }

    /// Layers a machine, a process and a filament into one preset.
    ///
    /// The order is the one the reference slicers apply: the machine first, then
    /// the material, then the print settings, so a process value wins over a
    /// filament value and both win over the machine's.
    pub fn compose(
        &self,
        machine: &str,
        process: &str,
        filament: &str,
    ) -> Result<Resolution, ConfigError> {
        let vendor = self.vendor_of_name(Kind::Machine, machine);
        let base = self.resolve_with(Kind::Machine, machine, vendor.as_deref())?;
        let material = self.resolve_with(Kind::Filament, filament, vendor.as_deref())?;
        let print = self.resolve_with(Kind::Process, process, vendor.as_deref())?;
        let mut preset = material.preset.with_base(base.preset());
        preset = print.preset.with_base(&preset);
        let mut warnings = base.warnings;
        warnings.extend(material.warnings);
        warnings.extend(print.warnings);
        Ok(Resolution { preset, warnings })
    }

    /// Applies a three-part job to a fresh configuration, reporting every key.
    ///
    /// The configuration keeps the machine's name, not the last preset layered
    /// over it: `name` is what the file's `; printer =` line and the profile name
    /// in a log mean, and a process preset's name there names the wrong thing.
    pub fn load(
        &self,
        machine: &str,
        process: &str,
        filament: &str,
    ) -> Result<Loaded, ConfigError> {
        let resolution = self.compose(machine, process, filament)?;
        let (config, report) = PrinterConfig::from_preset(resolution.preset());
        let config = PrinterConfig {
            name: machine.to_owned(),
            ..config
        };
        Ok(Loaded {
            config,
            report,
            resolution,
        })
    }

    fn walk<'a>(
        &'a self,
        kind: Kind,
        name: &str,
        prefer_vendor: Option<&str>,
        chain: &mut Vec<&'a Entry>,
        visiting: &mut Vec<String>,
    ) -> Result<(), ConfigError> {
        let path = format!("{}:{}", kind.label(), name);
        if visiting.contains(&path) {
            visiting.push(path);
            return Err(ConfigError::RecursivePreset {
                chain: visiting.clone(),
            });
        }
        let Some(entry) = self.pick(kind, name, prefer_vendor) else {
            let available = self.names(kind);
            return Err(ConfigError::MissingPreset {
                kind: kind.label(),
                name: name.to_owned(),
                where_: self.root.display().to_string(),
                available: available.iter().take(8).map(|n| (*n).to_owned()).collect(),
                total: available.len(),
            });
        };
        if chain.iter().any(|seen| std::ptr::eq(*seen, entry)) {
            return Ok(());
        }
        visiting.push(path);
        let vendor = entry.vendor.clone();
        for parent in &entry.parents {
            if self
                .pick(kind, parent, vendor.as_deref().or(prefer_vendor))
                .is_some()
            {
                self.walk(
                    kind,
                    parent,
                    vendor.as_deref().or(prefer_vendor),
                    chain,
                    visiting,
                )?;
            }
        }
        visiting.pop();
        chain.push(entry);
        Ok(())
    }

    /// The preset of that kind and name, preferring one under a named vendor.
    fn pick(&self, kind: Kind, name: &str, prefer_vendor: Option<&str>) -> Option<&Entry> {
        let matching = || {
            self.entries
                .iter()
                .filter(move |entry| entry.kind == kind && entry.name == name)
        };
        match prefer_vendor {
            Some(vendor) => matching()
                .find(|entry| entry.vendor.as_deref() == Some(vendor))
                .or_else(|| matching().next()),
            None => matching().next(),
        }
    }

    /// The vendor directory a named machine preset sits under, when it has one.
    pub fn vendor_of_machine(&self, machine: &str) -> Option<String> {
        self.vendor_of_name(Kind::Machine, machine)
    }

    fn lookup(&self, kind: Kind, name: &str) -> Option<&Entry> {
        self.pick(kind, name, None)
    }

    /// The vendor a named preset sits under, if any.
    fn vendor_of_name(&self, kind: Kind, name: &str) -> Option<String> {
        self.pick(kind, name, None)
            .and_then(|entry| entry.vendor.clone())
    }

    fn missing_parents(&self, kind: Kind, name: &str) -> Vec<String> {
        let mut missing: Vec<String> = Vec::new();
        let Some(entry) = self.lookup(kind, name) else {
            return missing;
        };
        let mut queue: Vec<&Entry> = vec![entry];
        while let Some(current) = queue.pop() {
            for parent in &current.parents {
                match self.lookup(current.kind, parent) {
                    Some(found) => {
                        if !queue.iter().any(|queued| std::ptr::eq(*queued, found))
                            && found.name != entry.name
                        {
                            queue.push(found);
                        }
                    }
                    None => missing.push(format!(
                        "{} `{}` inherits from `{}`, which is not in this bundle; \
                         the preset is applied without it",
                        current.kind.label(),
                        current.name,
                        parent
                    )),
                }
            }
        }
        missing
    }

    fn push(
        &mut self,
        kind: Kind,
        name: String,
        parents: Vec<String>,
        entries: Vec<(String, String)>,
        file: &Path,
    ) {
        self.entries.push(Entry {
            kind,
            name,
            parents,
            entries,
            vendor: self.vendor_of(file),
        });
    }

    /// The vendor directory a preset sits under.
    ///
    /// A slicer's shipped library is organised as vendor, kind, preset, and the
    /// same preset *name* appears under several vendors: every one of them has a
    /// "Generic PLA". Picking by name alone would silently apply another vendor's
    /// material, so the machine's vendor decides which one is meant.
    fn vendor_of(&self, file: &Path) -> Option<String> {
        let relative = file.strip_prefix(&self.root).ok()?;
        let mut parts = relative.components();
        let first = parts.next()?.as_os_str().to_str()?;
        parts.next()?;
        match Kind::parse(first) {
            Some(_) => None,
            None => Some(first.to_owned()),
        }
    }

    fn read_json(&mut self, file: &Path, text: &str) {
        let Some(pairs) = flat_json(text) else {
            self.warnings
                .push(format!("{}: not a flat JSON object", file.display()));
            return;
        };
        let field = |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let kind = field("type")
            .as_deref()
            .and_then(Kind::parse)
            .or_else(|| kind_from_path(file))
            .or_else(|| Kind::sniff(&pairs));
        let Some(kind) = kind else {
            self.warnings.push(format!(
                "{}: cannot tell whether this is a machine, process or filament preset",
                file.display()
            ));
            return;
        };
        let name = field("name")
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| stem(file));
        let parents = field("inherits")
            .map(|value| split_parents(&value))
            .unwrap_or_default();
        let entries: Vec<(String, String)> = pairs
            .into_iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "name" | "inherits" | "type" | "from" | "instantiation" | "setting_id"
                )
            })
            .collect();
        self.push(kind, name, parents, entries, file);
    }

    fn read_single(&mut self, file: &Path, text: &str) {
        let preset = match Preset::parse(text) {
            Ok(preset) => preset,
            Err(error) => {
                self.warnings.push(format!("{}: {error}", file.display()));
                return;
            }
        };
        let pairs: Vec<(String, String)> = preset.entries().to_vec();
        let kind = kind_from_path(file).or_else(|| Kind::sniff(&pairs));
        let Some(kind) = kind else {
            self.warnings.push(format!(
                "{}: cannot tell whether this is a machine, process or filament preset",
                file.display()
            ));
            return;
        };
        let name = match preset.name().is_empty() {
            true => stem(file),
            false => preset.name().to_owned(),
        };
        let parents = preset.inherits().map(split_parents).unwrap_or_default();
        self.push(kind, name, parents, pairs, file);
    }

    fn read_sections(&mut self, file: &Path, text: &str) {
        let mut blocks: Vec<Section> = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(header) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                blocks.push(
                    match header
                        .split_once(':')
                        .map(|(kind, name)| (Kind::parse(kind), name))
                    {
                        Some((Some(kind), name)) => {
                            (Some(kind), name.trim().to_owned(), Vec::new())
                        }
                        _ => (None, header.to_owned(), Vec::new()),
                    },
                );
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if let Some(block) = blocks.last_mut() {
                block
                    .2
                    .push((key.trim().to_owned(), value.trim().to_owned()));
            }
        }
        for (kind, name, mut entries) in blocks {
            let Some(kind) = kind else {
                continue;
            };
            let parents = entries
                .iter()
                .find(|(key, _)| key == "inherits")
                .map(|(_, value)| split_parents(value))
                .unwrap_or_default();
            entries.retain(|(key, _)| key != "inherits" && key != "name");
            self.push(
                kind_from_path(file).unwrap_or(kind),
                name,
                parents,
                entries,
                file,
            );
        }
    }
}

/// A composed bundle job, ready to slice.
#[derive(Clone, Debug)]
pub struct Loaded {
    config: PrinterConfig,
    report: IntakeReport,
    resolution: Resolution,
}

impl Loaded {
    /// The configuration the three presets were layered into.
    pub fn config(&self) -> &PrinterConfig {
        &self.config
    }

    /// Takes the configuration.
    pub fn into_config(self) -> PrinterConfig {
        self.config
    }

    /// How every key of the composed preset was handled.
    pub fn report(&self) -> &IntakeReport {
        &self.report
    }

    /// The composed preset and the parents it could not find.
    pub fn resolution(&self) -> &Resolution {
        &self.resolution
    }
}

fn is_json(file: &Path) -> bool {
    file.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn stem(file: &Path) -> String {
    file.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("preset")
        .to_owned()
}

/// Takes the kind from the directory a preset sits in, which is how a bundle
/// organised as `machine/`, `process/`, `filament/` names it.
fn kind_from_path(file: &Path) -> Option<Kind> {
    file.ancestors().skip(1).find_map(|directory| {
        directory
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(Kind::parse)
    })
}

fn starts_with_section(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with(';'))
        .is_some_and(|line| line.starts_with('[') && line.ends_with(']'))
}

/// Splits an `inherits` value, which lists parents separated by `;`.
fn split_parents(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(|name| name.trim().trim_matches('"').to_owned())
        .filter(|name| !name.is_empty() && name != "nil")
        .collect()
}

fn collect(directory: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<(), ConfigError> {
    if depth > 4 {
        return Ok(());
    }
    let read = std::fs::read_dir(directory).map_err(|error| ConfigError::Read {
        path: directory.display().to_string(),
        message: error.to_string(),
    })?;
    for entry in read {
        let entry = entry.map_err(|error| ConfigError::Read {
            path: directory.display().to_string(),
            message: error.to_string(),
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect(&path, depth + 1, out)?;
            continue;
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(extension.as_str(), "json" | "ini" | "cfg" | "config") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("hfw-bundle-{name}-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&root).expect("scratch directory");
            Scratch(root)
        }

        fn write(&self, relative: &str, body: &str) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("subdirectory");
            std::fs::write(&path, body).expect("preset file");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const ORCA_MACHINE: &str = r#"{
    "name": "Voron 2.4 350",
    "type": "machine",
    "from": "system",
    "instantiation": "true",
    "inherits": "Voron Base",
    "nozzle_diameter": ["0.4"],
    "printer_model": "Voron 2.4"
}"#;

    const ORCA_MACHINE_PARENT: &str = r#"{
    "name": "Voron Base",
    "type": "machine",
    "bed_shape": ["0x0", "350x0", "350x350", "0x350"],
    "max_layer_height": ["0.3"]
}"#;

    const ORCA_PROCESS: &str = r#"{
    "name": "0.20mm Standard",
    "type": "process",
    "inherits": "0.20mm Base",
    "layer_height": "0.2",
    "wall_loops": "4"
}"#;

    const ORCA_FILAMENT: &str = r#"{
    "name": "Generic PLA",
    "type": "filament",
    "inherits": "Generic Base",
    "nozzle_temperature": ["210"],
    "filament_type": ["PLA"]
}"#;

    fn orca_bundle(name: &str) -> (Scratch, Bundle) {
        let scratch = Scratch::new(name);
        scratch.write("Vendor/machine/voron.json", ORCA_MACHINE);
        scratch.write("Vendor/machine/voron-base.json", ORCA_MACHINE_PARENT);
        scratch.write("Vendor/process/standard.json", ORCA_PROCESS);
        scratch.write(
            "Vendor/process/base.json",
            r#"{"name":"0.20mm Base","type":"process","layer_height":"0.3","perimeters":"2"}"#,
        );
        scratch.write("Vendor/filament/pla.json", ORCA_FILAMENT);
        scratch.write(
            "Vendor/filament/pla-base.json",
            r#"{"name":"Generic Base","type":"filament","filament_density":["1.24"]}"#,
        );
        let bundle = Bundle::discover(&scratch.0).expect("bundle indexes");
        (scratch, bundle)
    }

    #[test]
    fn a_directory_of_json_presets_is_indexed_by_kind_and_name() {
        let (_scratch, bundle) = orca_bundle("index");
        assert_eq!(
            bundle.names(Kind::Machine),
            vec!["Voron 2.4 350", "Voron Base"]
        );
        assert_eq!(
            bundle.names(Kind::Process),
            vec!["0.20mm Base", "0.20mm Standard"]
        );
        assert_eq!(
            bundle.names(Kind::Filament),
            vec!["Generic Base", "Generic PLA"]
        );
    }

    #[test]
    fn a_chain_merges_parents_first() {
        let (_scratch, bundle) = orca_bundle("chain");
        let resolved = bundle
            .resolve(Kind::Process, "0.20mm Standard")
            .expect("process resolves");
        let pairs = resolved.preset().entries();
        let value = |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(value("layer_height"), Some("0.2"), "the child wins");
        assert_eq!(value("perimeters"), Some("2"), "the parent is still there");
        assert!(resolved.warnings().is_empty());
    }

    #[test]
    fn a_parent_outside_the_bundle_is_reported_and_skipped() {
        let (scratch, _bundle) = orca_bundle("missing-parent");
        scratch.write(
            "Vendor/filament/petg.json",
            r#"{"name":"Generic PETG","type":"filament","inherits":"Generic PETG @System","filament_type":["PETG"]}"#,
        );
        let bundle = Bundle::discover(&scratch.0).expect("bundle indexes");
        let resolved = bundle
            .resolve(Kind::Filament, "Generic PETG")
            .expect("a system parent does not stop the preset");
        assert_eq!(resolved.warnings().len(), 1);
        assert!(
            resolved.warnings()[0].contains("Generic PETG @System"),
            "{}",
            resolved.warnings()[0]
        );
        assert_eq!(
            resolved
                .preset()
                .entries()
                .iter()
                .find(|(key, _)| key == "filament_type")
                .map(|(_, value)| value.as_str()),
            Some("PETG")
        );
    }

    #[test]
    fn a_name_the_bundle_does_not_hold_names_what_it_does() {
        let (_scratch, bundle) = orca_bundle("missing-name");
        let error = bundle
            .resolve(Kind::Process, "0.20mm Standrad")
            .expect_err("a typo is an error");
        let text = error.to_string();
        assert!(text.contains("0.20mm Standrad"), "{text}");
        assert!(text.contains("0.20mm Standard"), "{text}");
    }

    #[test]
    fn compose_lets_the_process_win_over_the_filament_and_both_over_the_machine() {
        let (scratch, _bundle) = orca_bundle("compose");
        scratch.write(
            "Vendor/machine/shared.json",
            r#"{"name":"Shared Machine","type":"machine","nozzle_temperature":["200"],"wall_loops":"1","layer_height":"0.25"}"#,
        );
        scratch.write(
            "Vendor/filament/shared.json",
            r#"{"name":"Shared Filament","type":"filament","nozzle_temperature":["215"],"wall_loops":"2"}"#,
        );
        scratch.write(
            "Vendor/process/shared.json",
            r#"{"name":"Shared Process","type":"process","wall_loops":"6"}"#,
        );
        let bundle = Bundle::discover(&scratch.0).expect("bundle indexes");
        let loaded = bundle
            .load("Shared Machine", "Shared Process", "Shared Filament")
            .expect("compose");
        assert!(
            (loaded.config().nozzle_temp - 215.0).abs() < 1e-9,
            "filament over machine"
        );
        assert_eq!(loaded.config().walls, 6, "process over filament");
        assert!(
            (loaded.config().layer_height - 0.25).abs() < 1e-9,
            "machine survives"
        );
    }

    #[test]
    fn a_prusaslicer_vendor_file_is_read_as_one_preset_per_section() {
        let scratch = Scratch::new("sections");
        scratch.write(
            "VendorPrusa.ini",
            "# vendor profile\n\
             [vendor]\n\
             repo_id = Vendors\n\
             \n\
             [printer:Voron 2.4 350]\n\
             bed_shape = 0x0,350x0,350x350,0x350\n\
             nozzle_diameter = 0.4\n\
             \n\
             [print:0.20mm QUALITY]\n\
             layer_height = 0.2\n\
             perimeters = 5\n\
             inherits = 0.20mm Base\n\
             \n\
             [print:0.20mm Base]\n\
             layer_height = 0.3\n\
             perimeters = 2\n\
             \n\
             [filament:Prusament PLA]\n\
             filament_type = PLA\n\
             temperature = 215\n",
        );
        let bundle = Bundle::discover(&scratch.0).expect("bundle indexes");
        assert_eq!(bundle.names(Kind::Machine), vec!["Voron 2.4 350"]);
        assert_eq!(
            bundle.names(Kind::Process),
            vec!["0.20mm Base", "0.20mm QUALITY"]
        );
        assert_eq!(bundle.names(Kind::Filament), vec!["Prusament PLA"]);
        let resolved = bundle
            .resolve(Kind::Process, "0.20mm QUALITY")
            .expect("quality resolves");
        let pairs = resolved.preset().entries();
        assert_eq!(
            pairs
                .iter()
                .find(|(key, _)| key == "layer_height")
                .map(|(_, value)| value.as_str()),
            Some("0.2")
        );
        assert!(
            !pairs.iter().any(|(key, _)| key == "repo_id"),
            "a vendor header is not a preset"
        );
    }

    #[test]
    fn an_empty_directory_is_an_error_rather_than_an_empty_bundle() {
        let scratch = Scratch::new("empty");
        let error = Bundle::discover(&scratch.0).expect_err("no presets");
        assert!(error.to_string().contains("no presets"), "{error}");
    }

    #[test]
    fn a_kind_is_sniffed_from_a_preset_that_does_not_declare_one() {
        let scratch = Scratch::new("sniff");
        scratch.write("flat/plain.ini", "layer_height = 0.25\nperimeters = 3\n");
        scratch.write("flat/tool.ini", "nozzle_diameter = 0.6\n");
        scratch.write("flat/material.ini", "filament_type = ASA\n");
        let bundle = Bundle::discover(&scratch.0).expect("bundle indexes");
        assert_eq!(bundle.names(Kind::Process), vec!["plain"]);
        assert_eq!(bundle.names(Kind::Machine), vec!["tool"]);
        assert_eq!(bundle.names(Kind::Filament), vec!["material"]);
    }
}
