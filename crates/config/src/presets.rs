//! The preset surface a front end asks for: which machines a bundle holds, which
//! processes and filaments one machine accepts, and what a selection resolves to.
//!
//! **Resolution is one document, not a call per setting.** A panel holds the resolved
//! configuration, shows it, and hands the whole thing back as the profile a slice is
//! written with, so a per-key API would have it fetch some hundreds of values and then
//! reassemble exactly this document. One call, one blob, nothing to keep in step.
//!
//! **The generated surface answers, not a settings container of our own.** The key
//! names, the location each one is filed under and the type its value takes all come
//! from [`crate::surface_table`], which is generated from the same measurement the
//! ledger and the panel read. There is no second settings vocabulary here: the values
//! come from the preset the bundle resolves, and the names and shapes come from the
//! surface, so a panel cannot be offered a key this engine has never heard of, and this
//! engine cannot emit a key a panel cannot file.
//!
//! Two limits are deliberate and stated rather than hidden:
//!
//! - A setting the bundle names is emitted; one it does not is left to the panel's own
//!   library default, which is what a front end did before this existed. Emitting this
//!   engine's default for every unnamed key needs the inverse of `PrinterConfig::assign`
//!   for 324 fields, which is not written yet.
//! - One material, so filament blocks carry one slot: this engine prints with one
//!   extruder, and a one-entry array is the shape a panel already reads.

use crate::bundle::{Bundle, Kind};
use crate::error::ConfigError;
use crate::surface_table::{Row, ROWS};
use crate::PrinterConfig;

/// The machines a bundle holds, as JSON.
///
/// One entry per machine preset, and one printer profile inside it: the reference
/// slicers group several hardware configurations under one printer preset, while a
/// bundle here has one machine per preset. The fields are the ones a preset store
/// reads: `name` (what later queries are made with), `vendor_name`, `vendor_id`,
/// `technology`, and per profile its `name`, `extruders_cnt` and the `bed` it prints in.
pub fn printer_models(bundle: &Bundle) -> String {
    let mut json = String::from("{\"printer_models\":[");
    for (index, name) in bundle.names(Kind::Machine).into_iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        let bed = bed_of(bundle, name);
        let vendor = bundle
            .vendor_of_machine(name)
            .unwrap_or_else(|| "HFW".to_owned());
        json.push('{');
        text(&mut json, "name", name);
        json.push(',');
        text(&mut json, "vendor_name", &vendor);
        json.push(',');
        text(&mut json, "vendor_id", &vendor);
        json.push(',');
        text(&mut json, "technology", "FFF");
        json.push_str(",\"variants\":[{\"id\":");
        text_value(&mut json, &vendor);
        json.push_str(",\"printer_profiles\":[{\"name\":");
        text_value(&mut json, name);
        json.push_str(",\"extruders_cnt\":1,\"bed\":{");
        number(&mut json, "width", bed.0);
        json.push(',');
        number(&mut json, "height", bed.1);
        json.push(',');
        number(&mut json, "max_print_height", bed.2);
        json.push_str("}}]}]}");
    }
    json.push_str("]}");
    json
}

/// The profiles one machine accepts, as JSON.
///
/// Every process in the bundle, each listing every filament: a bundle here does not pair
/// a material with a process, and a panel reads the union of the per-process lists, so
/// listing them all gives the same set the pairing would.
pub fn profiles_for(bundle: &Bundle, printer: &str) -> Result<String, ConfigError> {
    if !bundle.names(Kind::Machine).contains(&printer) {
        return Err(missing(bundle, Kind::Machine, printer));
    }
    let filaments = bundle.names(Kind::Filament);
    let mut json = String::from("{\"print_profiles\":[");
    for (index, name) in bundle.names(Kind::Process).into_iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push('{');
        text(&mut json, "name", name);
        json.push_str(",\"filament_profiles\":[");
        for (position, filament) in filaments.iter().enumerate() {
            if position > 0 {
                json.push(',');
            }
            text_value(&mut json, filament);
        }
        json.push_str("]}");
    }
    json.push_str("]}");
    Ok(json)
}

/// One line saying what a bundle holds, for a log or a status bar.
pub fn summary(bundle: &Bundle) -> String {
    let mut line = bundle.summary();
    if !bundle.warnings().is_empty() {
        line.push_str(&format!(", {} warning(s)", bundle.warnings().len()));
    }
    line
}

/// What a machine, a process and a material resolve to, as one document.
///
/// `{"preset": {...}, "configuration": {"print_settings": {...},
/// "filament_settings": {...}, "printer_settings": {...}}, "warnings": [...]}` — the
/// shape a preset store already parses: the metadata of what was asked for, and every
/// setting the engine will slice with, filed where a panel keeps it.
///
/// The machine is required; a process or a material the bundle does not hold is a
/// warning rather than a failure, because a front end asking about a machine it holds
/// partly is a question worth answering. An empty material list means the machine and
/// process alone.
pub fn resolved_config(
    bundle: &Bundle,
    printer: &str,
    process: &str,
    filaments: &[&str],
) -> Result<String, ConfigError> {
    let machine = bundle.resolve(Kind::Machine, printer)?;
    let mut warnings: Vec<String> = machine.warnings().to_vec();
    let mut merged = machine.preset().clone();

    let filament = filaments
        .iter()
        .find(|name| !name.trim().is_empty())
        .copied();
    if let Some(name) = filament {
        match bundle.resolve(Kind::Filament, name) {
            Ok(resolution) => {
                warnings.extend(resolution.warnings().iter().cloned());
                merged = resolution.preset().clone().with_base(&merged);
            }
            Err(error) => warnings.push(error.to_string()),
        }
    }
    if !process.trim().is_empty() {
        match bundle.resolve(Kind::Process, process) {
            Ok(resolution) => {
                warnings.extend(resolution.warnings().iter().cloned());
                merged = resolution.preset().clone().with_base(&merged);
            }
            Err(error) => warnings.push(error.to_string()),
        }
    }

    let (config, report) = PrinterConfig::from_preset(&merged);
    warnings.extend(report.warnings().iter().cloned());

    let derived: Vec<(&str, String)> = vec![
        ("printer_model", quoted(printer)),
        ("bed_x", format!("{}", config.bed_x)),
        ("bed_y", format!("{}", config.bed_y)),
        ("max_print_height", format!("{}", config.max_z)),
        ("bed_shape", bed_shape_json(&config)),
    ];

    let mut emitted: Vec<(&'static str, &'static str, String)> = Vec::new();
    for (key, value) in merged.entries() {
        let Some(row) = row_for(key) else {
            continue;
        };
        if row.status != "applied" && row.status != "partial" {
            continue;
        }
        let block = match row.location {
            "Print" => "print",
            "Filament" => "filament",
            "Printer" | "Tool" => "printer",
            _ => continue,
        };
        if derived.iter().any(|(name, _)| *name == row.name) {
            continue;
        }
        let typed = typed_value(row, value);
        if typed.is_empty() {
            continue;
        }
        let typed = if row.location == "Filament" {
            format!("[{typed}]")
        } else {
            typed
        };
        emitted.push((block, row.name, typed));
    }
    for (name, value) in derived {
        emitted.push(("printer", name, value));
    }

    let settings_block = |block: &str| -> String {
        let mut out = String::new();
        for (_, name, value) in emitted
            .iter()
            .filter(|(block_name, _, _)| *block_name == block)
        {
            if !out.is_empty() {
                out.push(',');
            }
            text_value(&mut out, name);
            out.push(':');
            out.push_str(value);
        }
        out
    };
    let print = settings_block("print");
    let material = settings_block("filament");
    let machine_settings = settings_block("printer");

    let mut json = String::from("{\"preset\":{");
    text(&mut json, "name", printer);
    json.push(',');
    text(&mut json, "printer", printer);
    json.push(',');
    text(&mut json, "print", process);
    json.push_str(",\"filaments\":[");
    for (index, name) in filaments
        .iter()
        .filter(|name| !name.trim().is_empty())
        .enumerate()
    {
        if index > 0 {
            json.push(',');
        }
        text_value(&mut json, name);
    }
    json.push_str("]},\"configuration\":{\"print_settings\":{");
    json.push_str(&print);
    json.push_str("},\"filament_settings\":{");
    json.push_str(&material);
    json.push_str("},\"printer_settings\":{");
    json.push_str(&machine_settings);
    json.push_str("}},\"warnings\":[");
    for (index, warning) in warnings.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        text_value(&mut json, warning);
    }
    json.push_str("]}");
    Ok(json)
}

/// The surface row a preset key names: its own spelling first, then this engine's.
fn row_for(key: &str) -> Option<&'static Row> {
    ROWS.iter().find(|row| row.name == key).or_else(|| {
        ROWS.iter()
            .find(|row| !row.hfw.is_empty() && row.hfw == key)
    })
}

/// A setting's text as the JSON value its type takes.
///
/// A panel assigns a value by type: a number definition refuses text it cannot read as
/// a number, and a percentage arrives as `{"value": 85, "is_percent": true}`. Getting
/// this wrong does not fail loudly -- the panel keeps its own default and the user sees
/// a value the engine is not going to use -- so the typing follows the surface's own
/// record of the type and unit, and an empty or unreadable value is dropped rather than
/// guessed at.
fn typed_value(row: &Row, value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    let percent = |text: &str| -> String {
        let trimmed = text.trim().trim_end_matches('%');
        match trimmed.parse::<f64>() {
            Ok(number) if number.is_finite() => {
                format!("{{\"value\":{number},\"is_percent\":true}}")
            }
            _ => String::new(),
        }
    };
    let number = |text: &str| -> String {
        match text.trim().parse::<f64>() {
            Ok(number) if number.is_finite() => format!("{number}"),
            _ => String::new(),
        }
    };
    match row.kind {
        "bool" => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "enabled" => "true".to_owned(),
            "0" | "false" | "no" | "off" | "disabled" | "none" => "false".to_owned(),
            _ => String::new(),
        },
        "int" => match value.parse::<i64>() {
            Ok(number) => format!("{number}"),
            Err(_) => number(value),
        },
        "double" => number(value),
        "Percentage" => {
            if value.ends_with('%') {
                percent(value)
            } else {
                match value.parse::<f64>() {
                    Ok(share) if share.is_finite() => percent(&format!("{}", share * 100.0)),
                    _ => String::new(),
                }
            }
        }
        "FloatOrPercentage" => {
            if value.ends_with('%') {
                percent(value)
            } else {
                number(value)
            }
        }
        "vec<double>" | "optional<int>" => {
            let mut json = String::from("[");
            for (index, part) in value.split(',').enumerate() {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let typed = number(part);
                if typed.is_empty() {
                    return String::new();
                }
                if index > 0 && json.len() > 1 {
                    json.push(',');
                }
                json.push_str(&typed);
            }
            json.push(']');
            json
        }
        "vec<str>" => {
            let mut json = String::from("[");
            for (index, part) in value.split(',').enumerate() {
                if index > 0 {
                    json.push(',');
                }
                text_value(&mut json, part.trim());
            }
            json.push(']');
            json
        }
        "vec<Vec2d>" => {
            let mut json = String::from("[");
            for (index, pair) in value.split(',').enumerate() {
                let mut parts = pair.split('x');
                let (Some(x), Some(y)) = (parts.next(), parts.next()) else {
                    return String::new();
                };
                let (Ok(x), Ok(y)) = (x.trim().parse::<f64>(), y.trim().parse::<f64>()) else {
                    return String::new();
                };
                if index > 0 {
                    json.push(',');
                }
                json.push_str(&format!("[{x},{y}]"));
            }
            json.push(']');
            json
        }
        _ => quoted(value),
    }
}

fn bed_of(bundle: &Bundle, machine: &str) -> (f64, f64, f64) {
    match bundle.resolve(Kind::Machine, machine) {
        Ok(resolution) => {
            let (config, _) = PrinterConfig::from_preset(resolution.preset());
            (config.bed_x, config.bed_y, config.max_z)
        }
        Err(_) => (0.0, 0.0, 0.0),
    }
}

fn bed_shape_json(config: &PrinterConfig) -> String {
    let points: Vec<(f64, f64)> = if config.bed_shape.is_empty() {
        vec![
            (0.0, 0.0),
            (config.bed_x, 0.0),
            (config.bed_x, config.bed_y),
            (0.0, config.bed_y),
        ]
    } else {
        config.bed_shape.clone()
    };
    let mut json = String::from("[");
    for (index, (x, y)) in points.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str(&format!("[{x},{y}]"));
    }
    json.push(']');
    json
}

fn missing(bundle: &Bundle, kind: Kind, name: &str) -> ConfigError {
    let available = bundle.names(kind);
    ConfigError::MissingPreset {
        kind: kind.label(),
        name: name.to_owned(),
        where_: bundle.root().display().to_string(),
        available: available
            .iter()
            .take(8)
            .map(|item| (*item).to_owned())
            .collect(),
        total: available.len(),
    }
}

fn text(out: &mut String, key: &str, value: &str) {
    text_value(out, key);
    out.push(':');
    text_value(out, value);
}

fn quoted(value: &str) -> String {
    let mut out = String::new();
    text_value(&mut out, value);
    out
}

fn text_value(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
}

fn number(out: &mut String, key: &str, value: f64) {
    text_value(out, key);
    out.push(':');
    out.push_str(&format!("{value}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "hfw-presets-{name}-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).expect("scratch directory");
            Scratch(root)
        }

        fn write(&self, relative: &str, body: &str) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("subdirectory");
            std::fs::write(&path, body).expect("preset file");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn bundle(name: &str) -> (Scratch, Bundle) {
        let scratch = Scratch::new(name);
        scratch.write(
            "Vendor/machine/voron.ini",
            "[machine:Voron 2.4 350]\n\
             bed_x = 350\n\
             bed_y = 350\n\
             max_z = 330\n\
             nozzle_diameter = 0.4\n\
             flavor = klipper\n",
        );
        scratch.write(
            "Vendor/process/standard.ini",
            "[process:0.20mm Standard]\n\
             layer_height = 0.2\n\
             walls = 4\n\
             infill_density = 0.35\n\
             outer_wall_speed = 120\n\
             sparse_infill_density = 25%\n\
             ironing = 1\n",
        );
        scratch.write(
            "Vendor/filament/pla.ini",
            "[filament:Generic PLA]\n\
             nozzle_temp = 210\n\
             bed_temp = 60\n",
        );
        scratch.write(
            "Vendor/filament/petg.ini",
            "[filament:Generic PETG]\n\
             nozzle_temp = 240\n",
        );
        let bundle = Bundle::discover(&scratch.0).expect("bundle indexes");
        (scratch, bundle)
    }

    #[test]
    fn the_printer_list_names_every_machine_with_its_bed() {
        let (_scratch, bundle) = bundle("printers");
        let json = printer_models(&bundle);
        assert!(json.starts_with("{\"printer_models\":["), "{json}");
        assert!(json.contains("\"name\":\"Voron 2.4 350\""), "{json}");
        assert!(json.contains("\"vendor_name\":\"Vendor\""), "{json}");
        assert!(json.contains("\"technology\":\"FFF\""), "{json}");
        assert!(json.contains("\"extruders_cnt\":1"), "{json}");
        assert!(json.contains("\"width\":350"), "{json}");
        assert!(json.contains("\"height\":350"), "{json}");
        assert!(json.contains("\"max_print_height\":330"), "{json}");
        assert!(
            json.contains("\"printer_profiles\":[{\"name\":\"Voron 2.4 350\""),
            "a machine is its own hardware configuration: {json}"
        );
    }

    #[test]
    fn the_profile_list_offers_every_process_and_every_material() {
        let (_scratch, bundle) = bundle("profiles");
        let json = profiles_for(&bundle, "Voron 2.4 350").expect("the machine is in the bundle");
        assert!(json.contains("{\"name\":\"0.20mm Standard\""), "{json}");
        assert!(
            json.contains("\"filament_profiles\":[\"Generic PETG\",\"Generic PLA\"]"),
            "every material in name order: {json}"
        );
        let error = profiles_for(&bundle, "Prusa MK4").expect_err("a machine it does not hold");
        assert!(
            error.to_string().contains("Prusa MK4"),
            "the refusal names the machine: {error}"
        );
    }

    #[test]
    fn a_resolved_configuration_carries_the_names_locations_and_types_a_panel_files_by() {
        let (_scratch, bundle) = bundle("resolved");
        let json = resolved_config(
            &bundle,
            "Voron 2.4 350",
            "0.20mm Standard",
            &["Generic PLA"],
        )
        .expect("the selection resolves");
        assert!(json.starts_with("{\"preset\":{"), "{json}");
        assert!(json.contains("\"printer\":\"Voron 2.4 350\""), "{json}");
        assert!(json.contains("\"filaments\":[\"Generic PLA\"]"), "{json}");
        assert!(json.contains("\"print_settings\":{"), "{json}");
        assert!(json.contains("\"filament_settings\":{"), "{json}");
        assert!(json.contains("\"printer_settings\":{"), "{json}");
        assert!(
            json.contains("\"layer_height\":0.2"),
            "a number is a number, not text: {json}"
        );
        assert!(
            json.contains("\"perimeters\":4"),
            "a setting is named the way the panel's own definitions name it: {json}"
        );
        assert!(
            json.contains("\"ironing\":true"),
            "a flag is a boolean: {json}"
        );
        assert!(
            json.contains("\"gcode_flavor\":\"klipper\""),
            "a word stays a string: {json}"
        );
        assert!(
            json.contains("\"max_print_height\":330"),
            "the machine's own numbers come from the resolved configuration: {json}"
        );
        assert_eq!(
            json.matches("\"max_print_height\":").count(),
            1,
            "a derived value does not appear twice: {json}"
        );
        assert!(
            json.contains("\"bed_shape\":[[0,0],[350,0],[350,350],[0,350]]"),
            "{json}"
        );
        assert!(
            json.contains("\"temperature\":[210]"),
            "a material value is one slot per tool: {json}"
        );
        assert!(
            json.contains("\"bed_temperature\":[60]"),
            "and every material setting is: {json}"
        );
        assert!(
            !json.contains("\"outer_wall_speed\""),
            "a key no panel box carries is not filed somewhere wrong: {json}"
        );
    }

    #[test]
    fn a_percentage_is_written_as_the_panel_reads_one() {
        let (_scratch, bundle) = bundle("percent");
        let json = resolved_config(
            &bundle,
            "Voron 2.4 350",
            "0.20mm Standard",
            &["Generic PLA"],
        )
        .expect("the selection resolves");
        assert!(
            json.contains("\"fill_density\":{\"value\":35,\"is_percent\":true}"),
            "a density this engine holds as a share is a percentage to a panel: {json}"
        );
    }

    #[test]
    fn a_material_or_process_the_bundle_lacks_is_a_warning_not_a_failure() {
        let (_scratch, bundle) = bundle("partial");
        let json = resolved_config(&bundle, "Voron 2.4 350", "0.28mm Draft", &["Silk PLA"])
            .expect("a machine it holds still resolves");
        assert!(json.contains("\"warnings\":["), "{json}");
        assert!(
            json.contains("0.28mm Draft"),
            "the warning names the process: {json}"
        );
        assert!(json.contains("Silk PLA"), "and the material: {json}");
        assert!(
            json.contains("\"max_print_height\":330"),
            "the machine's own numbers still resolve: {json}"
        );
    }

    #[test]
    fn a_machine_the_bundle_does_not_hold_is_refused_by_name() {
        let (_scratch, bundle) = bundle("missing");
        let error = resolved_config(&bundle, "Prusa MK4", "0.20mm Standard", &[])
            .expect_err("a machine it does not hold");
        assert!(error.to_string().contains("Prusa MK4"), "{error}");
    }

    #[test]
    fn the_summary_counts_the_bundle_and_its_warnings() {
        let (_scratch, bundle) = bundle("summary");
        let line = summary(&bundle);
        assert!(line.contains("1 machine(s)"), "{line}");
        assert!(line.contains("1 process(es)"), "{line}");
        assert!(line.contains("2 filament(s)"), "{line}");
    }
}
