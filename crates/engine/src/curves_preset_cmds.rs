//! Curves presets: the Curves dialog's Preset menu, its built-in curves, the user's saved ones and
//! Photoshop `.acv` files (Save Preset… / Load Preset…).
//!
//! A preset stores its curves the way an `.acv` file does: the composite first, then each channel
//! of the colour model, as `[input, output]` points in 0..=255. Applying one reads those curves in
//! the colour model of the document ([`params_for`]), so the same preset works in RGB, Grayscale,
//! CMYK and Lab, as a Photoshop preset does. User presets persist with the other presets in the
//! preferences document. The built-in curves were designed here (clean-room), named after the kind
//! of tone change they make.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::adjust_params;
use crate::commands::CommandSpec;
use crate::presets::{always, bad, req_str};
use crate::{Result, Session};
use photocraft_doc::ColorMode;
use photocraft_doc::adjust::ToneSpace;

/// Most user presets kept.
pub const MAX_PRESETS: usize = 256;
/// The preset menu's first two entries, which are not presets.
pub const DEFAULT: &str = "Default";
pub const CUSTOM: &str = "Custom";

/// A named set of curves, composite first (`.acv` channel order), `[input, output]` in 0..=255.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvesPreset {
    pub name: String,
    pub curves: Vec<Vec<[u16; 2]>>,
}

fn line() -> Vec<[u16; 2]> {
    vec![[0, 0], [255, 255]]
}

/// The built-in presets, in menu order.
pub fn builtin() -> Vec<CurvesPreset> {
    let one = |name: &str, c: &[[u16; 2]]| CurvesPreset { name: name.into(), curves: vec![c.to_vec()] };
    vec![
        CurvesPreset {
            name: "Cross Process (RGB)".into(),
            curves: vec![line(), vec![[0, 0], [64, 46], [192, 214], [255, 255]], vec![[0, 0], [64, 54], [192, 206], [255, 255]], vec![[0, 34], [255, 218]]],
        },
        one("Darker (RGB)", &[[0, 0], [140, 112], [255, 255]]),
        one("Increase Contrast (RGB)", &[[0, 0], [64, 58], [192, 198], [255, 255]]),
        one("Lighter (RGB)", &[[0, 0], [112, 140], [255, 255]]),
        one("Linear Contrast (RGB)", &[[0, 0], [32, 18], [224, 238], [255, 255]]),
        one("Medium Contrast (RGB)", &[[0, 0], [64, 52], [192, 204], [255, 255]]),
        one("Negative (RGB)", &[[0, 255], [255, 0]]),
        one("Strong Contrast (RGB)", &[[0, 0], [64, 42], [192, 214], [255, 255]]),
    ]
}

/// Whether Levels/Curves edit a document of `mode` through one Gray channel.
fn is_gray(mode: ColorMode) -> bool {
    matches!(mode, ColorMode::Grayscale | ColorMode::Duotone | ColorMode::Bitmap)
}

/// The Curves params keys of a preset's curves, in `.acv` order, for a document of `mode`.
pub fn curve_keys(mode: ColorMode) -> &'static [&'static str] {
    if is_gray(mode) {
        return &["points"];
    }
    match adjust_params::default_space(mode) {
        ToneSpace::Rgb => &["points", "red", "green", "blue"],
        ToneSpace::Cmyk => &["points", "cyan", "magenta", "yellow", "black"],
        ToneSpace::Lab => &["lightness", "a", "b"],
    }
}

/// `image.adjustments.curves` params applying `curves` to a document of `mode`: missing curves
/// are straight lines, extra ones are ignored.
pub fn params_for(curves: &[Vec<[u16; 2]>], mode: ColorMode) -> Result<Value> {
    let mut p = json!({"pencil": false});
    for (i, key) in curve_keys(mode).iter().enumerate() {
        let c = curves.get(i).filter(|c| c.len() >= 2).cloned().unwrap_or_else(line);
        p[*key] = json!(c.iter().map(|q| [q[0].min(255), q[1].min(255)]).collect::<Vec<_>>());
    }
    let adjustment = adjust_params::from_params("curves", &p, None, mode)?;
    let mut v = adjust_params::to_params(&adjustment);
    v["pencil"] = json!(false);
    Ok(v)
}

/// The curves of Curves params `p` (point or pencil-drawn) in a document of `mode`, in `.acv`
/// order. Pencil-drawn curves are fitted with points, as Photoshop's Save Preset does.
pub fn curves_of(p: &Value, mode: ColorMode) -> Result<Vec<Vec<[u16; 2]>>> {
    let mut p = p.clone();
    if !p.is_object() {
        return Err(bad("curves.presets", "`params` must be an object"));
    }
    p["pencil"] = json!(false);
    let adjustment = adjust_params::from_params("curves", &p, None, mode)?;
    let mode = match &adjustment {
        photocraft_doc::Adjustment::Curves { space: ToneSpace::Cmyk, .. } => ColorMode::Cmyk,
        photocraft_doc::Adjustment::Curves { space: ToneSpace::Lab, .. } => ColorMode::Lab,
        _ if is_gray(mode) => mode,
        _ => ColorMode::Rgb,
    };
    let v = adjust_params::to_params(&adjustment);
    let level = |x: &Value| x.as_f64().filter(|x| x.is_finite()).map(|x| x.round().clamp(0.0, 255.0) as u16);
    Ok(curve_keys(mode)
        .iter()
        .map(|key| {
            let pts: Vec<[u16; 2]> = v
                .get(*key)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|q| Some([level(q.get(0)?)?, level(q.get(1)?)?])).collect())
                .unwrap_or_default();
            if pts.len() < 2 { line() } else { pts }
        })
        .collect())
}

/// An `.acv` file of `curves`, padded to the five curves Photoshop writes.
pub fn to_acv(curves: &[Vec<[u16; 2]>]) -> Vec<u8> {
    let mut all = curves.to_vec();
    while all.len() < 5 {
        all.push(line());
    }
    photocraft_psd::acv::write(&all)
}

fn checked_name(name: &str) -> std::result::Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > crate::brush_preset_cmds::MAX_NAME {
        return Err("Preset names must contain 1–255 characters after trimming".into());
    }
    if [DEFAULT, CUSTOM].iter().any(|n| n.eq_ignore_ascii_case(name)) || builtin().iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
        return Err(format!("\"{name}\" is a built-in preset name"));
    }
    Ok(name.into())
}

fn valid_curves(curves: &[Vec<[u16; 2]>]) -> bool {
    !curves.is_empty()
        && curves.len() <= photocraft_psd::acv::MAX_CURVES
        && curves.iter().all(|c| (2..=photocraft_psd::acv::MAX_POINTS).contains(&c.len()) && c.iter().all(|q| q[0] <= 255 && q[1] <= 255))
}

/// Read entries independently so a damaged preset cannot discard the others.
pub(crate) fn load(value: Value) -> Vec<CurvesPreset> {
    let Some(entries) = value.as_array() else { return Vec::new() };
    let mut presets: Vec<CurvesPreset> = Vec::new();
    for entry in entries.iter().take(MAX_PRESETS) {
        let Ok(mut preset) = serde_json::from_value::<CurvesPreset>(entry.clone()) else { continue };
        let Ok(name) = checked_name(&preset.name) else { continue };
        if !valid_curves(&preset.curves) || presets.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
            continue;
        }
        preset.name = name;
        presets.push(preset);
    }
    presets
}

fn doc_mode(s: &Session) -> ColorMode {
    s.active().map_or(ColorMode::Rgb, |d| d.doc.mode)
}

/// Adds or replaces (same name, any case) a user preset.
fn store(s: &mut Session, cmd: &str, preset: CurvesPreset) -> Result<()> {
    let list = &mut s.presets.curves;
    if let Some(existing) = list.iter_mut().find(|p| p.name.eq_ignore_ascii_case(&preset.name)) {
        *existing = preset;
    } else if list.len() >= MAX_PRESETS {
        return Err(bad(cmd, format!("At most {MAX_PRESETS} curves presets can be saved")));
    } else {
        list.push(preset);
    }
    s.presets_changed();
    Ok(())
}

/// A preset by name: the user's first, then the built-ins. Returns it and whether it is the user's.
pub fn find(s: &Session, name: &str) -> Option<(CurvesPreset, bool)> {
    let name = name.trim();
    if let Some(p) = s.presets.curves.iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
        return Some((p.clone(), true));
    }
    builtin().into_iter().find(|p| p.name.eq_ignore_ascii_case(name)).map(|p| (p, false))
}

fn get(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "curves.presets.get";
    let name = req_str(p, "name", CMD)?;
    let (preset, user) = find(s, name).ok_or_else(|| bad(CMD, "No such curves preset"))?;
    let params = params_for(&preset.curves, doc_mode(s))?;
    Ok(json!({"preset": preset, "user": user, "params": params}))
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "curves.presets.save";
    let name = checked_name(req_str(p, "name", CMD)?).map_err(|e| bad(CMD, e))?;
    let curves = match (p.get("params"), p.get("curves")) {
        (Some(params), None) => curves_of(params, doc_mode(s))?,
        (None, Some(c)) => {
            let c: Vec<Vec<[u16; 2]>> = serde_json::from_value(c.clone()).map_err(|e| bad(CMD, format!("`curves`: {e}")))?;
            let c: Vec<Vec<[u16; 2]>> = c
                .into_iter()
                .map(|mut c| {
                    c.sort_by_key(|q| q[0]);
                    c.dedup_by_key(|q| q[0]);
                    c
                })
                .collect();
            if !valid_curves(&c) {
                return Err(bad(CMD, "`curves` must hold 1–32 curves of 2–256 [input, output] points in 0..=255"));
            }
            c
        }
        _ => return Err(bad(CMD, "give either `params` (Curves params) or `curves`")),
    };
    let preset = CurvesPreset { name, curves };
    store(s, CMD, preset.clone())?;
    Ok(json!({"preset": preset}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "curves.presets.delete";
    let name = req_str(p, "name", CMD)?;
    let index = s.presets.curves.iter().position(|pr| pr.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| bad(CMD, "No such user curves preset"))?;
    s.presets.curves.remove(index);
    s.presets_changed();
    Ok(json!({"deleted": name}))
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "curves.presets.import";
    let (bytes, stem) = crate::preset_import_cmds::file_bytes(p, CMD, ".acv file")?;
    let curves = photocraft_psd::acv::parse(&bytes).map_err(|e| bad(CMD, format!("not a readable curves file (.acv): {e}")))?;
    let base = match p.get("name") {
        Some(v) => v.as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(CMD, "`name` must be a non-empty string"))?.to_string(),
        None if !stem.trim().is_empty() => stem.trim().to_string(),
        None => "Curves".to_string(),
    };
    // A file named like a built-in becomes "Name 2" rather than shadowing it.
    let mut name = base.clone();
    let mut n = 2;
    while checked_name(&name).is_err() && n < 1000 {
        name = format!("{base} {n}");
        n += 1;
    }
    let name = checked_name(&name).map_err(|e| bad(CMD, e))?;
    let preset = CurvesPreset { name, curves };
    store(s, CMD, preset.clone())?;
    let params = params_for(&preset.curves, doc_mode(s))?;
    Ok(json!({"preset": preset, "params": params}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "curves.presets.export";
    let curves = match (p.get("name").and_then(Value::as_str), p.get("params")) {
        (Some(name), None) => find(s, name).ok_or_else(|| bad(CMD, "No such curves preset"))?.0.curves,
        (None, Some(params)) => curves_of(params, doc_mode(s))?,
        _ => return Err(bad(CMD, "give either `name` (a preset) or `params` (Curves params)")),
    };
    Ok(json!({"data": photocraft_paint::tile::b64_encode(&to_acv(&curves))}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "curves.presets.list",
            label: "List Curves Presets",
            menu: &[],
            shortcut: None,
            params: r#"{} -> {builtin:[{name,curves}], user:[{name,curves}]} (curves: composite first, [input,output] 0..255)"#,
            enabled: always,
            run: |s, _| Ok(json!({"builtin": builtin(), "user": s.presets.curves})),
            journal: false,
        },
        CommandSpec {
            id: "curves.presets.get",
            label: "Get Curves Preset",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str} -> {preset, user:bool, params} (params for image.adjustments.curves in the active document's mode)"#,
            enabled: always,
            run: get,
            journal: false,
        },
        CommandSpec {
            id: "curves.presets.save",
            label: "Save Curves Preset",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str, "params":{curves params} | "curves":[[[in,out],…],…]} (a saved name is replaced)"#,
            enabled: always,
            run: save,
            journal: false,
        },
        CommandSpec {
            id: "curves.presets.delete",
            label: "Delete Curves Preset",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str} (user presets only)"#,
            enabled: always,
            run: delete,
            journal: false,
        },
        CommandSpec {
            id: "curves.presets.import",
            label: "Load Curves Preset",
            menu: &[],
            shortcut: None,
            params: r#"{"path":str | "data":base64, "name"?:str} -> {preset, params} (a Photoshop .acv file; saved as a user preset)"#,
            enabled: always,
            run: import,
            journal: false,
        },
        CommandSpec {
            id: "curves.presets.export",
            label: "Export Curves Preset",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str | "params":{curves params}} -> {data:base64} (a Photoshop .acv file)"#,
            enabled: always,
            run: export,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acv(curves: &[&[[u16; 2]]]) -> String {
        photocraft_paint::tile::b64_encode(&to_acv(&curves.iter().map(|c| c.to_vec()).collect::<Vec<_>>()))
    }

    #[test]
    fn import_applies_in_the_document_mode_and_persists() {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        let data = acv(&[&[[0, 0], [128, 160], [255, 255]], &[[0, 20], [255, 255]]]);
        let r = s.execute("curves.presets.import", json!({"data": data, "name": "Warm"})).unwrap();
        assert_eq!(r["params"]["points"], json!([[0.0, 0.0], [128.0, 160.0], [255.0, 255.0]]));
        assert_eq!(r["params"]["red"], json!([[0.0, 20.0], [255.0, 255.0]]));
        assert_eq!(r["params"]["blue"], json!([[0.0, 0.0], [255.0, 255.0]]));
        // It applies through the real command.
        s.execute("image.adjustments.curves", r["params"].clone()).unwrap();

        let raw = s.prefs_to_json();
        let mut restored = Session::new();
        restored.load_prefs_json(&raw).unwrap();
        let got = restored.execute("curves.presets.get", json!({"name": "warm"})).unwrap();
        assert_eq!(got["user"], json!(true));
        assert_eq!(got["preset"]["curves"][0], json!([[0, 0], [128, 160], [255, 255]]));
        restored.execute("curves.presets.delete", json!({"name": "Warm"})).unwrap();
        assert!(restored.presets.curves.is_empty());
    }

    #[test]
    fn save_and_export_round_trip_through_acv() {
        let mut s = Session::new();
        let params = json!({"points": [[0, 0], [100, 150], [255, 255]], "green": [[0, 0], [255, 200]]});
        s.execute("curves.presets.save", json!({"name": "Mine", "params": params})).unwrap();
        let data = s.execute("curves.presets.export", json!({"name": "Mine"})).unwrap()["data"].as_str().unwrap().to_string();
        let bytes = photocraft_paint::tile::b64_decode(&data).unwrap();
        let curves = photocraft_psd::acv::parse(&bytes).unwrap();
        assert_eq!(curves.len(), 5, "Photoshop's five RGB curves");
        assert_eq!(curves[0], vec![[0, 0], [100, 150], [255, 255]]);
        assert_eq!(curves[2], vec![[0, 0], [255, 200]]);
        // Pencil-drawn params are fitted with points.
        let map: Vec<f32> = (0..256).map(|k| 255.0 - k as f32).collect();
        let fitted = curves_of(&json!({"pencil": true, "points": map}), ColorMode::Rgb).unwrap();
        assert!(fitted[0].first() == Some(&[0, 255]) && fitted[0].last() == Some(&[255, 0]), "{fitted:?}");
    }

    #[test]
    fn presets_follow_the_colour_mode() {
        let c = vec![vec![[0, 0], [64, 100], [255, 255]], vec![[0, 10], [255, 255]]];
        let gray = params_for(&c, ColorMode::Grayscale).unwrap();
        assert_eq!(gray["points"][1], json!([64.0, 100.0]));
        let cmyk = params_for(&c, ColorMode::Cmyk).unwrap();
        assert_eq!(cmyk["cyan"][0], json!([0.0, 10.0]));
        let lab = params_for(&c, ColorMode::Lab).unwrap();
        assert_eq!(lab["lightness"][1], json!([64.0, 100.0]));
        assert_eq!(lab["a"][0], json!([0.0, 10.0]));
        assert_eq!(curves_of(&lab, ColorMode::Lab).unwrap()[0], c[0]);
        for p in builtin() {
            assert!(valid_curves(&p.curves), "{}", p.name);
            params_for(&p.curves, ColorMode::Rgb).unwrap();
        }
    }

    #[test]
    fn bad_params_are_errors_not_panics() {
        let mut s = Session::new();
        for (id, p) in [
            ("curves.presets.save", json!({})),
            ("curves.presets.save", json!({"name": "x"})),
            ("curves.presets.save", json!({"name": "  ", "curves": [[[0, 0], [255, 255]]]})),
            ("curves.presets.save", json!({"name": "Default", "curves": [[[0, 0], [255, 255]]]})),
            ("curves.presets.save", json!({"name": "darker (rgb)", "curves": [[[0, 0], [255, 255]]]})),
            ("curves.presets.save", json!({"name": "x", "curves": [[[0, 0]]]})),
            ("curves.presets.save", json!({"name": "x", "curves": [[[0, 0], [999, 255]]]})),
            ("curves.presets.save", json!({"name": "x", "curves": "no"})),
            ("curves.presets.save", json!({"name": "x", "params": 3})),
            ("curves.presets.delete", json!({"name": "Darker (RGB)"})),
            ("curves.presets.get", json!({"name": "missing"})),
            ("curves.presets.import", json!({"data": "!!"})),
            ("curves.presets.import", json!({"data": photocraft_paint::tile::b64_encode(&[0, 4])})),
            ("curves.presets.import", json!({"data": acv(&[&[[0, 0], [255, 255]]]), "name": 4})),
            ("curves.presets.export", json!({})),
            ("curves.presets.export", json!({"name": "missing"})),
        ] {
            assert!(s.execute(id, p.clone()).is_err(), "{id} {p}");
        }
        // A file named like a built-in does not shadow it.
        let r = s.execute("curves.presets.import", json!({"data": acv(&[&[[0, 0], [255, 255]]]), "name": "Negative (RGB)"})).unwrap();
        assert_eq!(r["preset"]["name"], "Negative (RGB) 2");
        assert_eq!(find(&s, "Negative (RGB)").map(|p| p.1), Some(false));
    }

    #[test]
    fn a_damaged_saved_entry_is_dropped() {
        let loaded = load(json!([
            {"name": "ok", "curves": [[[0, 0], [255, 255]]]},
            {"name": "bad", "curves": [[[0, 0]]]},
            {"name": "Default", "curves": [[[0, 0], [255, 255]]]},
            {"name": "OK", "curves": [[[0, 0], [255, 255]]]},
            "nope"
        ]));
        assert_eq!(loaded.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["ok"]);
    }
}
