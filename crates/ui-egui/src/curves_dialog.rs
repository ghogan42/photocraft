//! Image › Adjustments › Curves laid out as Photoshop's dialog: the Preset menu and its gear
//! (Save Preset…, Load Preset…, Delete Current Preset) above a group holding the Channel menu, the
//! curve tools, the graph with its histogram, the black and white input sliders, Input / Output and
//! the eyedroppers; Show Amount of, Grid size and Show in the middle; OK, Cancel, Smooth, Auto and
//! Preview on the right. The Properties panel keeps its compact editor (`adjust_editors::curves`).
//!
//! The display options are view state (`UiState::curves_display`), remembered between sessions.
//! Presets go through the `curves.presets.*` commands; `.acv` files are Photoshop's format.

use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use photocraft_doc::adjust::ToneSpace;
use photocraft_engine::adjust_params;
use photocraft_engine::curves_preset_cmds::{self as presets, CUSTOM, DEFAULT};
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::adjust_editors::{self as ed, Edit, ToneChannel};
use crate::point_curve as curve_edit;
use crate::state::{CurvesDisplayState, CurvesEditorState};
use crate::theme::Tokens;
use crate::tone::{self, HistSource, Histograms};
use crate::widgets;

/// The selected Preset menu entry (a preset name, "Default" or "Custom").
pub const PRESET: &str = "__preset";
/// Whether [`PRESET`] names one of the user's presets (Delete Current Preset applies).
pub const PRESET_USER: &str = "__presetUser";

/// Graph side, as Photoshop's 256-level grid.
const GRAPH: f32 = 256.0;
/// Room left of the graph for the curve tools and Output.
const GUTTER: f32 = 64.0;
/// The vertical gradient bar and its gap to the graph.
const VBAR: f32 = 14.0;
/// Inner padding of a group box.
const PAD: f32 = 10.0;
const MIDDLE: f32 = 168.0;
const BUTTONS: f32 = 92.0;
const COLUMN_GAP: f32 = 14.0;
const HANDLE: f32 = 7.0;
const LEFT: f32 = GUTTER + VBAR + GRAPH + 2.0 * PAD;
/// The dialog's content width (the frame sizes itself to it).
pub const WIDTH: f32 = LEFT + COLUMN_GAP + MIDDLE + COLUMN_GAP + BUTTONS;

/// Root of the dialog's editor state in egui memory (shared with `adjust_dialog`'s tests).
fn mem() -> egui::Id {
    egui::Id::new(("adjust-dialog", "curves"))
}

enum PresetAction {
    Save,
    Load,
    Delete(String),
}

/// What the dialog body needs to draw the channel group.
struct GraphCx<'a> {
    hist: Option<&'a Histograms>,
    gray: bool,
    isolated: Option<&'a photocraft_engine::channel_cmds::IsolatedCurveChannel>,
    display: &'a CurvesDisplayState,
    pigment: bool,
}

/// The colour model key of [`CurvesDisplayState::pigment`].
fn model(gray: bool, space: ToneSpace) -> &'static str {
    match space {
        _ if gray => "gray",
        ToneSpace::Rgb => "rgb",
        ToneSpace::Cmyk => "cmyk",
        ToneSpace::Lab => "lab",
    }
}

/// Dialog body. Returns OK (`Some(true)`) or Cancel (`Some(false)`): this dialog owns its buttons.
pub fn body(app: &mut PhotocraftApp, ui: &mut Ui, fields: &mut Map<String, Value>, id: u64, interactive: bool) -> Option<bool> {
    let mode = crate::adjust_dialog::curve_mode(app, "curves");
    let mut values = crate::filter_dialog::params_of(fields);
    let active = app.session.active().and_then(|s| s.active_layer);
    let isolated = crate::adjust_dialog::isolated(app);
    let gray_plane = isolated.as_ref().and_then(|c| c.gray);
    let gray = gray_plane.is_some() || app.session.active().is_some_and(|s| ed::is_gray(s.doc.mode));
    let hist = active.map(|layer| {
        let source = match gray_plane {
            Some(surface) => HistSource::Channel { layer, surface },
            None => HistSource::Layer(layer),
        };
        tone::histograms(app, source, ed::space_of(&values))
    });
    let space = ed::space_of(&values);
    let mut display = app.ui.curves_display.clone();
    let pigment = display.pigment_for(model(gray, space));
    let mut preview = fields.get("__preview").and_then(Value::as_bool).unwrap_or(true);
    let mut e = Edit::default();
    let mut preset_applied = false;
    let mut action = None;
    let mut outcome = None;
    let mut reset = false;
    {
        let app: &PhotocraftApp = app;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
            ui.vertical(|ui| {
                ui.set_width(LEFT);
                if let Some((name, user, params)) = preset_row(app, ui, fields, mode, &mut action) {
                    set_preset(fields, &name, user);
                    if let Some(p) = params {
                        values = p;
                        preset_applied = true;
                        e.add(Edit::discrete(true));
                    }
                }
                ui.add_space(8.0);
                let cx = GraphCx { hist: hist.as_deref(), gray, isolated: isolated.as_ref(), display: &display, pigment };
                let (edit, key) = channel_group(ui, &mut values, &cx, fields, mode);
                e.add(edit);
                ui.data_mut(|d| d.insert_temp(mem().with("curves-key"), key));
            });
            ui.vertical(|ui| {
                ui.set_width(MIDDLE);
                display_options(ui, &mut display, model(gray, space), isolated.is_none() && !gray);
            });
            ui.vertical(|ui| {
                ui.set_width(BUTTONS);
                ui.spacing_mut().item_spacing.y = 8.0;
                if widgets::primary_button(ui, tl!("OK"), BUTTONS).clicked() {
                    outcome = Some(true);
                }
                // Photoshop: holding Alt turns Cancel into Reset (the dialog stays open).
                let alt = ui.input(|i| i.modifiers.alt);
                if widgets::secondary_button(ui, if alt { tl!("Reset") } else { tl!("Cancel") }, BUTTONS).clicked() {
                    if alt {
                        reset = true;
                    } else {
                        outcome = Some(false);
                    }
                }
                let pencil = values.get("pencil").and_then(Value::as_bool).unwrap_or(false);
                let label = crate::i18n::tr_ctx(crate::i18n::current(), "curves", "Smooth");
                let smooth = ui
                    .add_enabled_ui(pencil, |ui| widgets::secondary_button(ui, label, BUTTONS))
                    .inner
                    .on_hover_text(tl!("Smooth the curve values"))
                    .on_disabled_hover_text(tl!("Smooth the curve values"));
                if smooth.clicked() {
                    let key: &'static str = ui.data(|d| d.get_temp(mem().with("curves-key"))).unwrap_or("points");
                    let mut map = ed::read_map(&values, key);
                    adjust_params::smooth_curve_map(&mut map);
                    ed::write_map(&mut values, key, &map);
                    e.add(Edit::discrete(true));
                }
                let can_auto = hist.is_some() && (gray || space == ToneSpace::Rgb);
                let auto = ui
                    .add_enabled_ui(can_auto, |ui| widgets::secondary_button(ui, tl!("Auto"), BUTTONS))
                    .inner
                    .on_hover_text(tl!("Enhance Per Channel Contrast"));
                if auto.clicked()
                    && let Some(h) = hist.as_deref()
                {
                    e.add(Edit::discrete(auto_curves(&mut values, h, gray, isolated.as_ref())));
                }
                ui.add_space(4.0);
                widgets::checkbox(ui, &mut preview, "Preview");
            });
        });
    }
    if display != app.ui.curves_display {
        app.ui.curves_display = display;
    }
    fields.insert("__preview".into(), json!(preview));
    if reset {
        crate::adjust_dialog::reset(app, ui.ctx(), fields);
        set_preset(fields, DEFAULT, false);
    } else if e.changed
        && let Value::Object(v) = values
    {
        if !preset_applied {
            set_preset(fields, CUSTOM, false);
        }
        // Replace the parameter keys (an edit may remove one), keep the private ones.
        fields.retain(|k, _| k.starts_with("__"));
        fields.extend(v);
        crate::adjust_dialog::rebase_picker(fields);
    }
    if let Some(action) = action {
        run_preset_action(app, fields, id, mode, action);
    }
    if outcome.is_none() && interactive && !egui::Popup::is_any_open(ui.ctx()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        outcome = Some(true);
    }
    outcome
}

fn set_preset(fields: &mut Map<String, Value>, name: &str, user: bool) {
    fields.insert(PRESET.into(), json!(name));
    fields.insert(PRESET_USER.into(), json!(user));
}

/// Auto: Photoshop's default Enhance Per Channel Contrast, clipping 0.1 % at each end of each
/// channel's histogram (Gray, or the targeted channel, alone). The composite is left as it is.
fn auto_curves(v: &mut Value, h: &Histograms, gray: bool, isolated: Option<&photocraft_engine::channel_cmds::IsolatedCurveChannel>) -> bool {
    if v.get("pencil").and_then(Value::as_bool) == Some(true) && !ed::set_pencil(v, false, gray) {
        return false;
    }
    let targets: Vec<(&str, usize)> = match isolated {
        Some(iso) => vec![(if iso.key.is_empty() { "points" } else { iso.key }, iso.hist)],
        None if gray => vec![("points", 0)],
        None => vec![("red", 1), ("green", 2), ("blue", 3)],
    };
    for (key, i) in targets {
        let (lo, hi) = ed::auto_levels(&h[i]);
        v[key] = json!([[lo, 0.0], [hi, 255.0]]);
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Presets

/// A Preset menu pick: the entry's name, whether it is the user's, and the params it applies
/// (none for Custom, which keeps the curve).
type Pick = (String, bool, Option<Value>);

/// Preset: menu and the gear.
fn preset_row(app: &PhotocraftApp, ui: &mut Ui, fields: &Map<String, Value>, mode: photocraft_doc::ColorMode, action: &mut Option<PresetAction>) -> Option<Pick> {
    let builtin = presets::builtin();
    let user = &app.session.presets.curves;
    let current = fields.get(PRESET).and_then(Value::as_str).unwrap_or(DEFAULT).to_string();
    let is_user = fields.get(PRESET_USER).and_then(Value::as_bool).unwrap_or(false);
    let first_user = 2 + builtin.len();
    let mut labels: Vec<(usize, String)> = vec![(0, tl!("Default").to_string()), (1, tl!("Custom").to_string())];
    labels.extend(builtin.iter().enumerate().map(|(i, p)| (2 + i, crate::i18n::t(&p.name).to_string())));
    labels.extend(user.iter().enumerate().map(|(i, p)| (first_user + i, p.name.clone())));
    let selected = if is_user {
        user.iter().position(|p| p.name.eq_ignore_ascii_case(&current)).map(|i| first_user + i)
    } else if current == DEFAULT {
        Some(0)
    } else {
        builtin.iter().position(|p| p.name == current).map(|i| 2 + i)
    };
    let mut chosen = selected.unwrap_or(1);
    let mut picked = false;
    ui.horizontal(|ui| {
        ed::label(ui, tl!("Preset:"));
        let opts: Vec<(usize, &str)> = labels.iter().map(|(i, l)| (*i, l.as_str())).collect();
        let width = LEFT - 96.0;
        picked = widgets::dropdown_literal(ui, "curves-preset", &mut chosen, &opts, width);
        let gear = crate::icons::button(ui, "settings", 24.0, false, tl!("Preset Options"));
        egui::Popup::menu(&gear).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            if ui.button(tl!("Save Preset…")).clicked() {
                *action = Some(PresetAction::Save);
                ui.close();
            }
            if ui.button(tl!("Load Preset…")).clicked() {
                *action = Some(PresetAction::Load);
                ui.close();
            }
            let delete = ui.add_enabled_ui(is_user, |ui| ui.button(tl!("Delete Current Preset"))).inner;
            if delete.clicked() {
                *action = Some(PresetAction::Delete(current.clone()));
                ui.close();
            }
        });
    });
    if !picked || Some(chosen) == selected {
        return None;
    }
    Some(match chosen {
        0 => (DEFAULT.to_string(), false, adjust_params::default_for("curves", mode).map(|a| adjust_params::to_params(&a)).ok()),
        1 => (CUSTOM.to_string(), false, None),
        i if i < first_user => {
            let p = builtin.get(i - 2)?;
            (p.name.clone(), false, presets::params_for(&p.curves, mode).ok())
        }
        i => {
            let p = user.get(i - first_user)?;
            (p.name.clone(), true, presets::params_for(&p.curves, mode).ok())
        }
    })
}

fn report(app: &mut PhotocraftApp, r: Result<Value, String>) {
    if let Err(error) = r
        && error != crate::file_dialog::CANCELLED
    {
        app.ui.status = error;
        app.ui.status_error = true;
    }
}

fn stem(path: &str) -> String {
    std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Curves".into())
}

fn run_preset_action(app: &mut PhotocraftApp, fields: &mut Map<String, Value>, id: u64, mode: photocraft_doc::ColorMode, action: PresetAction) {
    match action {
        PresetAction::Save => {
            let values = crate::filter_dialog::params_of(fields);
            let curves = match presets::curves_of(&values, mode) {
                Ok(c) => c,
                Err(error) => return report(app, Err(error.to_string())),
            };
            let bytes = presets::to_acv(&curves);
            let current = fields.get(PRESET).and_then(Value::as_str).filter(|_| fields.get(PRESET_USER).and_then(Value::as_bool) == Some(true)).unwrap_or("Curves");
            let r = app.pick_save(&format!("{current}.acv"), move |app, path| {
                let write = app.services.write.as_mut().ok_or("no writer configured")?;
                write(&path, &bytes)?;
                let name = stem(&path);
                let r = app.run("curves.presets.save", json!({"name": name, "curves": curves}))?;
                if let Some(d) = app.ui.dialog_mut(id) {
                    set_preset(&mut d.fields, &name, true);
                }
                Ok(r)
            });
            report(app, r);
        }
        PresetAction::Load => {
            let r = app.pick_file_bytes_filtered(&["acv"], move |app, name, bytes| {
                let r = app.run("curves.presets.import", json!({"data": photocraft_engine::paint::tile::b64_encode(&bytes), "name": stem(&name)}))?;
                apply_loaded(app, id, mode, &r)?;
                Ok(r)
            });
            report(app, r);
        }
        PresetAction::Delete(name) => {
            let r = app.run("curves.presets.delete", json!({"name": name}));
            if r.is_ok() {
                set_preset(fields, CUSTOM, false);
            }
            report(app, r);
        }
    }
}

/// Load Preset…: the imported preset's curves become the dialog's, in its colour mode.
fn apply_loaded(app: &mut PhotocraftApp, id: u64, mode: photocraft_doc::ColorMode, r: &Value) -> Result<(), String> {
    let curves: Vec<Vec<[u16; 2]>> = serde_json::from_value(r["preset"]["curves"].clone()).map_err(|e| e.to_string())?;
    let name = r["preset"]["name"].as_str().unwrap_or(CUSTOM).to_string();
    let Value::Object(params) = presets::params_for(&curves, mode).map_err(|e| e.to_string())? else { return Err("bad curves".into()) };
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields.retain(|k, _| k.starts_with("__"));
        d.fields.extend(params);
        set_preset(&mut d.fields, &name, true);
        crate::adjust_dialog::rebase_picker(&mut d.fields);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Layout helpers

/// Photoshop's group box: a hairline frame whose top edge runs through `title`.
fn group<R>(ui: &mut Ui, width: f32, title: impl FnOnce(&mut Ui), body: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    let r = ui.vertical(|ui| {
        ui.set_width(width);
        let head = ui
            .horizontal(|ui| {
                ui.add_space(PAD - 2.0);
                ui.scope(title).response.rect
            })
            .inner;
        ui.add_space(4.0);
        let inner = ui
            .horizontal(|ui| {
                ui.add_space(PAD);
                ui.vertical(|ui| {
                    ui.set_width(width - 2.0 * PAD);
                    body(ui)
                })
                .inner
            })
            .inner;
        ui.add_space(PAD);
        (head, inner)
    });
    let ((head, inner), rect) = (r.inner, r.response.rect);
    let y = head.center().y;
    let (x0, x1) = (rect.left(), rect.left() + width);
    let pts = vec![
        pos2(head.left() - 4.0, y),
        pos2(x0, y),
        pos2(x0, rect.bottom()),
        pos2(x1, rect.bottom()),
        pos2(x1, y),
        pos2((head.right() + 4.0).min(x1), y),
    ];
    ui.painter().add(egui::Shape::line(pts, Stroke::new(1.0, t.separator)));
    ui.add_space(8.0);
    inner
}

fn group_label(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!(text)).color(t.text).size(12.5));
}

/// A Photoshop radio button.
fn radio(ui: &mut Ui, on: bool, label: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let text = tl!(label).to_string();
    let r = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (r, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
            ui.painter().circle_stroke(r.center(), 6.0, Stroke::new(1.2, if on { t.accent } else { t.text_faint }));
            if on {
                ui.painter().circle_filled(r.center(), 3.2, t.accent);
            }
            let l = ui.add(egui::Label::new(egui::RichText::new(&text).color(t.text_dim)).selectable(false).sense(Sense::click()));
            resp.union(l)
        })
        .inner;
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, &text));
    r
}

/// A grid-size button: a square glyph split into `cells` × `cells`.
fn grid_button(ui: &mut Ui, cells: u32, selected: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    let ink = crate::icons::button_chrome(ui, rect, selected, resp.hovered());
    let g = Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
    let p = ui.painter();
    p.rect_stroke(g, 0.0, Stroke::new(1.0, ink), StrokeKind::Inside);
    for i in 1..cells {
        let f = i as f32 / cells as f32;
        p.line_segment([pos2(g.left() + f * g.width(), g.top()), pos2(g.left() + f * g.width(), g.bottom())], Stroke::new(1.0, ink));
        p.line_segment([pos2(g.left(), g.top() + f * g.height()), pos2(g.right(), g.top() + f * g.height())], Stroke::new(1.0, ink));
    }
    let tip = tl!(tip).to_string();
    let resp = resp.on_hover_text(&tip);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &tip));
    resp
}

/// Show Amount of, Grid size and Show.
fn display_options(ui: &mut Ui, d: &mut CurvesDisplayState, model: &str, overlays: bool) {
    let pigment = d.pigment_for(model);
    group(
        ui,
        MIDDLE,
        |ui| group_label(ui, "Show Amount of:"),
        |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.add_space(2.0);
            if radio(ui, !pigment, "Light  (0-255)").clicked() {
                d.pigment.insert(model.to_string(), false);
            }
            if radio(ui, pigment, "Pigment/Ink %").clicked() {
                d.pigment.insert(model.to_string(), true);
            }
        },
    );
    group(
        ui,
        MIDDLE,
        |ui| group_label(ui, "Grid size:"),
        |ui| {
            ui.horizontal(|ui| {
                if grid_button(ui, 2, !d.fine_grid, "Simple grid: 25% increments").clicked() {
                    d.fine_grid = false;
                }
                if grid_button(ui, 5, d.fine_grid, "Detailed grid: 10% increments").clicked() {
                    d.fine_grid = true;
                }
            });
        },
    );
    group(
        ui,
        MIDDLE,
        |ui| group_label(ui, "Show:"),
        |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_space(2.0);
            ui.add_enabled_ui(overlays, |ui| widgets::checkbox(ui, &mut d.channel_overlays, "Channel Overlays"));
            widgets::checkbox(ui, &mut d.histogram, "Histogram");
            widgets::checkbox(ui, &mut d.baseline, "Baseline");
            widgets::checkbox(ui, &mut d.intersection_line, "Intersection Line");
        },
    );
}

// ---------------------------------------------------------------------------------------------
// The channel group: tools, graph, input sliders, Input / Output, eyedroppers

/// `[input, output]` as shown: Pigment/Ink % runs both axes from 0 % ink (white) to 100 %.
fn flip(q: [f32; 2], pigment: bool) -> [f32; 2] {
    if pigment { [255.0 - q[0], 255.0 - q[1]] } else { q }
}

fn flip_points(pts: &[[f32; 2]], pigment: bool) -> Vec<[f32; 2]> {
    if pigment { pts.iter().rev().map(|q| flip(*q, true)).collect() } else { pts.to_vec() }
}

/// A pencil map as shown (and back: flipping is its own inverse).
fn flip_map(map: &[f32], pigment: bool) -> Vec<f32> {
    if !pigment {
        return map.to_vec();
    }
    let n = map.len();
    (0..n).map(|k| 255.0 - map.get(n - 1 - k).copied().unwrap_or(k as f32)).collect()
}

fn channel_group(ui: &mut Ui, v: &mut Value, cx: &GraphCx, fields: &mut Map<String, Value>, mode: photocraft_doc::ColorMode) -> (Edit, &'static str) {
    let space = ed::space_of(v);
    let chans = ed::tone_channels(space, cx.gray);
    let ch = std::cell::Cell::new(0usize);
    let mut out = (Edit::default(), "points");
    group(
        ui,
        LEFT,
        |ui| {
            ch.set(match cx.isolated {
                Some(iso) => ed::channel_picker_opts(ui, mem().with("curves-ch"), &[(0, iso.name.as_str())], tl!("Channel:"), iso.translate),
                None => ed::channel_picker(ui, mem().with("curves-ch"), chans, tl!("Channel:")),
            });
        },
        |ui| {
            out = graph_block(ui, v, cx, ch.get(), chans);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(GUTTER + VBAR);
                crate::adjust_dialog::picker_controls(ui, fields, mode, false);
            });
        },
    );
    out
}

fn graph_block(ui: &mut Ui, v: &mut Value, cx: &GraphCx, ch: usize, chans: &[ToneChannel]) -> (Edit, &'static str) {
    let t = Tokens::get(ui.ctx());
    let pigment = cx.pigment;
    let state_id = mem().with("curves");
    let mut st: CurvesEditorState = ui.data(|d| d.get_temp(state_id)).unwrap_or_default();
    // A new channel or a flipped axis starts without a selected point.
    let flipped_id = mem().with("curves-pigment");
    if ch != st.channel || ui.data(|d| d.get_temp::<bool>(flipped_id)).is_some_and(|p| p != pigment) {
        st = CurvesEditorState { channel: ch, ..Default::default() };
    }
    ui.data_mut(|d| d.insert_temp(flipped_id, pigment));
    let chan = match cx.isolated {
        Some(iso) => ToneChannel { key: iso.key, label: "" },
        None => chans.get(ch).copied().unwrap_or(ToneChannel { key: "", label: "" }),
    };
    let tinted = cx.isolated.is_some_and(|c| c.tint) || (cx.isolated.is_none() && ch != 0);
    let hist_i = cx.isolated.map_or(ch, |c| c.hist);
    let key = ed::curve_key(&chan);
    let mut e = Edit::default();
    let mut pencil = v.get("pencil").and_then(Value::as_bool).unwrap_or(false);

    let (block, _) = ui.allocate_exact_size(vec2(GUTTER + VBAR + GRAPH, GRAPH + 50.0), Sense::hover());
    let graph = Rect::from_min_size(block.min + vec2(GUTTER + VBAR, 0.0), vec2(GRAPH, GRAPH));
    let vbar = Rect::from_min_size(block.min + vec2(GUTTER, 0.0), vec2(8.0, GRAPH));
    let hbar = Rect::from_min_max(pos2(graph.left(), graph.bottom() + 4.0), pos2(graph.right(), graph.bottom() + 12.0));
    let sliders = Rect::from_min_max(pos2(graph.left() - 6.0, hbar.bottom() + 1.0), pos2(graph.right() + 6.0, hbar.bottom() + 12.0));
    ui.data_mut(|d| d.insert_temp(mem().with("curves-graph"), graph));
    let to_scr = |q: [f32; 2]| pos2(graph.left() + q[0] / 255.0 * graph.width(), graph.bottom() - q[1] / 255.0 * graph.height());

    // Curve tools, top left: edit points, draw with the pencil.
    ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_min_size(block.min, vec2(GUTTER - 4.0, 30.0))), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if crate::icons::button(ui, "spline", 28.0, !pencil, tl!("Edit points to modify the curve")).clicked() && pencil {
                e.add(Edit::discrete(ed::set_pencil(v, false, cx.gray)));
            }
            if crate::icons::button(ui, "pencil", 28.0, pencil, tl!("Draw to modify the curve")).clicked() && !pencil {
                e.add(Edit::discrete(ed::set_pencil(v, true, cx.gray)));
            }
        });
    });
    pencil = v.get("pencil").and_then(Value::as_bool).unwrap_or(false);
    let pts = ed::read_curve(v, key);
    let mut disp = flip_points(&pts, pigment);
    let mut map = flip_map(&ed::read_map(v, key), pigment);
    let mut changed = false;
    let mut drawn = false;

    let resp = ui.interact(graph.expand(5.0), mem().with("curves-graph-area"), Sense::click_and_drag());
    if pencil {
        // A stroke draws while the button is down and commits on release; Shift continues from
        // the previous stroke's end in a straight line.
        let stroke_id = mem().with("curves-pencil");
        let (mut active, mut last): (bool, Option<[f32; 2]>) = ui.data(|d| d.get_temp(stroke_id)).unwrap_or_default();
        if resp.is_pointer_button_down_on()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let q = [
                ((pos.x - graph.left()) / graph.width().max(1.0) * 255.0).clamp(0.0, 255.0),
                ((graph.bottom() - pos.y) / graph.height().max(1.0) * 255.0).clamp(0.0, 255.0),
            ];
            let from = if active || ui.input(|i| i.modifiers.shift) { last.unwrap_or(q) } else { q };
            drawn = curve_edit::pencil_line(&mut map, from, q);
            active = true;
            last = Some(q);
        } else if active {
            active = false;
            e.commit = true;
        }
        ui.data_mut(|d| d.insert_temp(stroke_id, (active, last)));
    } else {
        let interaction = curve_edit::interact(ui, &resp, graph, &mut disp, &mut st.gesture);
        changed |= interaction.changed;
        e.commit |= interaction.commit;
        // The black and white input sliders under the gradient bar move the end points' inputs.
        let slider = ui.interact(sliders, mem().with("curves-io-sliders"), Sense::drag());
        let grab_id = mem().with("curves-io-grab");
        if slider.drag_started()
            && let Some(pos) = slider.interact_pointer_pos()
        {
            let xs = [disp.first(), disp.last()].map(|q| q.map_or(0.0, |q| to_scr(*q).x));
            let i = if (pos.x - xs[0]).abs() <= (pos.x - xs[1]).abs() { 0usize } else { 1 };
            ui.data_mut(|d| d.insert_temp(grab_id, i));
        }
        if slider.dragged()
            && let Some(pos) = slider.interact_pointer_pos()
        {
            let end = ui.data(|d| d.get_temp::<usize>(grab_id)).unwrap_or(0);
            let i = if end == 0 { 0 } else { disp.len().saturating_sub(1) };
            let x = ((pos.x - graph.left()) / graph.width().max(1.0) * 255.0).clamp(0.0, 255.0);
            if let Some(q) = disp.get(i).copied() {
                changed |= curve_edit::move_to(&mut disp, i, [x, q[1]]);
            }
        }
        if slider.drag_stopped() {
            e.commit = true;
        }
    }

    // Output (left of the graph's foot) and Input (under the sliders) of the selected point.
    let selected = st.gesture.selected.filter(|i| *i < disp.len() && !pencil);
    let (unit_max, suffix) = if pigment { (100.0, "%") } else { (255.0, "") };
    let shown = |x: f32| if pigment { (x / 255.0 * 100.0).round() } else { x };
    let (mut vi, mut vo) = selected.and_then(|i| disp.get(i)).map_or((0.0, 0.0), |q| (shown(q[0]), shown(q[1])));
    let output_box = Rect::from_min_size(pos2(block.left(), graph.bottom() - 46.0), vec2(GUTTER - 6.0, 46.0));
    let ro = ui
        .scope_builder(egui::UiBuilder::new().max_rect(output_box), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ed::label(ui, tl!("Output:"));
            ui.add_enabled_ui(selected.is_some(), |ui| widgets::value_field(ui, &mut vo, 0.0..=unit_max, suffix, 52.0)).inner
        })
        .inner;
    let input_box = Rect::from_min_size(pos2(graph.left(), sliders.bottom() + 4.0), vec2(GRAPH, 24.0));
    let ri = ui
        .scope_builder(egui::UiBuilder::new().max_rect(input_box), |ui| {
            ui.horizontal(|ui| {
                ed::label(ui, tl!("Input:"));
                ui.add_enabled_ui(selected.is_some(), |ui| widgets::value_field(ui, &mut vi, 0.0..=unit_max, suffix, 52.0)).inner
            })
            .inner
        })
        .inner;
    if let Some(i) = selected
        && (ri.changed() || ro.changed())
    {
        let level = |x: f32| if pigment { x / 100.0 * 255.0 } else { x };
        if curve_edit::move_to(&mut disp, i, [level(vi), level(vo)]) {
            changed = true;
            e.commit |= !(ri.dragged() || ro.dragged());
        }
    }
    if ri.drag_stopped() || ro.drag_stopped() {
        e.commit = true;
    }

    // Draw: histogram, grid, baseline, gradient bars, overlays, intersection, the curve, points.
    let p = ui.painter_at(block.expand(6.0));
    p.rect_filled(graph, 0.0, t.field);
    if cx.display.histogram
        && let Some(h) = cx.hist
    {
        let mut bins = h[hist_i];
        if pigment {
            bins.reverse();
        }
        tone::draw_histogram(&p, graph, &bins, Color32::from_gray(if t.pro { 88 } else { 190 }).gamma_multiply(0.75));
    }
    let cells = if cx.display.fine_grid { 10 } else { 4 };
    for i in 1..cells {
        let f = i as f32 / cells as f32;
        let g = Stroke::new(1.0, t.separator);
        p.line_segment([pos2(graph.left() + f * graph.width(), graph.top()), pos2(graph.left() + f * graph.width(), graph.bottom())], g);
        p.line_segment([pos2(graph.left(), graph.top() + f * graph.height()), pos2(graph.right(), graph.top() + f * graph.height())], g);
    }
    if cx.display.baseline {
        p.line_segment([graph.left_bottom(), graph.right_top()], Stroke::new(1.0, t.separator.gamma_multiply(1.6)));
    }
    p.rect_stroke(
        graph,
        0.0,
        Stroke::new(if resp.has_focus() { 1.5 } else { 1.0 }, if resp.has_focus() { t.accent } else { t.field_border }),
        StrokeKind::Outside,
    );
    let bar = ed::bar_color(&chan, &t);
    let (dark, light) = if pigment { (bar, Color32::BLACK) } else { (Color32::BLACK, bar) };
    ed::gradient_bar(&p, hbar, dark, light, false);
    ed::gradient_bar(&p, vbar, dark, light, true);
    let draw_points = |pts: &[[f32; 2]], color: Color32, width: f32| {
        let cp: Vec<photocraft_doc::adjust::CurvePoint> =
            pts.iter().map(|q| photocraft_doc::adjust::CurvePoint { input: q[0] / 255.0, output: q[1] / 255.0 }).collect();
        let lut = photocraft_compose::adjust::curve_lut(&cp);
        let n = lut.len().max(2);
        let line: Vec<Pos2> = (0..n)
            .step_by((n / 256).max(1))
            .map(|i| to_scr(flip([i as f32 / (n - 1) as f32 * 255.0, lut.get(i).copied().unwrap_or(0.0) * 255.0], pigment)))
            .collect();
        p.add(egui::Shape::line(line, Stroke::new(width, color)));
    };
    let draw_map = |map: &[f32], color: Color32, width: f32| {
        let last = map.len().saturating_sub(1).max(1) as f32;
        let line: Vec<Pos2> = map.iter().enumerate().map(|(k, o)| to_scr([k as f32 / last * 255.0, *o])).collect();
        p.add(egui::Shape::line(line, Stroke::new(width, color)));
    };
    if cx.display.channel_overlays && cx.isolated.is_none() && ch == 0 {
        // The edited channel curves over the composite.
        for c in chans.iter().skip(1) {
            if pencil {
                let m = ed::read_map(v, c.key);
                if m.iter().enumerate().any(|(k, o)| (o - k as f32).abs() > 0.5) {
                    draw_map(&flip_map(&m, pigment), ed::bar_color(c, &t).gamma_multiply(0.8), 1.0);
                }
                continue;
            }
            let cp = ed::read_curve(v, c.key);
            if cp.len() > 2 || cp.first().is_some_and(|q| q[1] != 0.0) || cp.last().is_some_and(|q| q[1] != 255.0) {
                draw_points(&cp, ed::bar_color(c, &t).gamma_multiply(0.8), 1.0);
            }
        }
    }
    let unflipped = flip_points(&disp, pigment);
    if cx.display.intersection_line
        && !pencil
        && let Some(i) = st.gesture.drag.and_then(|d| d.index)
        && let Some(q) = disp.get(i)
    {
        let s = to_scr(*q);
        let stroke = Stroke::new(1.0, t.text_dim.gamma_multiply(0.7));
        p.line_segment([pos2(graph.left(), s.y), pos2(graph.right(), s.y)], stroke);
        p.line_segment([pos2(s.x, graph.top()), pos2(s.x, graph.bottom())], stroke);
    }
    let color = if tinted { bar } else { t.text };
    if pencil {
        draw_map(&map, color, 1.5);
    } else {
        draw_points(&unflipped, color, 1.5);
        for (i, q) in disp.iter().enumerate() {
            let r = Rect::from_center_size(to_scr(*q), vec2(HANDLE, HANDLE));
            if Some(i) == st.gesture.selected {
                p.rect_filled(r, 0.0, t.text);
                p.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Outside);
            } else {
                p.rect_filled(r, 0.0, t.field);
                p.rect_stroke(r, 0.0, Stroke::new(1.0, t.text), StrokeKind::Inside);
            }
        }
        // The input sliders: the dark end's triangle and the light end's.
        let (left, right) = if pigment { (Color32::WHITE, Color32::BLACK) } else { (Color32::BLACK, Color32::WHITE) };
        for (q, fill) in [(disp.first(), left), (disp.last(), right)] {
            let Some(q) = q else { continue };
            let x = to_scr(*q).x;
            let tri = vec![pos2(x, sliders.top()), pos2(x + 5.5, sliders.bottom() - 1.0), pos2(x - 5.5, sliders.bottom() - 1.0)];
            p.add(egui::Shape::convex_polygon(tri, fill, Stroke::new(1.0, t.text_dim)));
        }
    }
    ui.data_mut(|d| d.insert_temp(state_id, st));

    if drawn {
        ed::write_map(v, key, &flip_map(&map, pigment));
        if space_is_lab(v)
            && let Some(o) = v.as_object_mut()
        {
            o.remove("points");
        }
        e.changed = true;
    }
    if changed && !pencil {
        v[key] = json!(unflipped.iter().map(|q| [q[0].round(), q[1].round()]).collect::<Vec<_>>());
        if space_is_lab(v)
            && let Some(o) = v.as_object_mut()
        {
            // Lab has no composite: "points" would alias lightness.
            o.remove("points");
        }
        e.changed = true;
    }
    (e, key)
}

fn space_is_lab(v: &Value) -> bool {
    ed::space_of(v) == ToneSpace::Lab
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bright.acv as other tools write it (version 5): one composite curve, four straight lines.
    fn bright_acv() -> Vec<u8> {
        let words: [u16; 33] = [5, 5, 5, 0, 0, 17, 12, 58, 50, 132, 128, 255, 255, 2, 0, 0, 255, 255, 2, 0, 0, 255, 255, 2, 0, 0, 255, 255, 2, 0, 0, 255, 255];
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    /// Gear › Load Preset… applies an .acv and lists it as the user's; Save Preset… writes an .acv
    /// and lists it; Delete Current Preset removes it and leaves the curve as Custom.
    #[test]
    fn load_save_and_delete_presets_through_the_gear() {
        use crate::file_dialog::FileDialogAnswer;
        use std::cell::RefCell;
        use std::rc::Rc;
        type Written = Rc<RefCell<Vec<(String, Vec<u8>)>>>;
        let written: Written = Rc::default();
        let log = written.clone();
        let (dialog, _) = crate::file_dialog::fake(vec![
            Some(FileDialogAnswer::Contents("/presets/Bright.acv".into(), bright_acv())),
            Some(FileDialogAnswer::SaveTo("/presets/Mine.acv".into())),
        ]);
        let services = crate::Services {
            file_dialog: Some(dialog),
            write: Some(Box::new(move |path: &str, bytes: &[u8]| {
                log.borrow_mut().push((path.to_string(), bytes.to_vec()));
                Ok(())
            })),
            ..Default::default()
        };
        let mut session = photocraft_engine::Session::new();
        session.execute("file.new", json!({"width": 16, "height": 16})).unwrap();
        let mut app = PhotocraftApp::new(session, services);
        let id = crate::adjust_dialog::open(&mut app, "image.adjustments.curves").unwrap();
        let ctx = egui::Context::default();
        let fields = |app: &mut PhotocraftApp| app.ui.dialog_mut(id).unwrap().fields.clone();
        assert_eq!(fields(&mut app)[PRESET], json!(DEFAULT));

        let mut f = fields(&mut app);
        run_preset_action(&mut app, &mut f, id, photocraft_doc::ColorMode::Rgb, PresetAction::Load);
        app.poll_file_dialog(&ctx, None);
        let f = fields(&mut app);
        assert_eq!(f["points"][1], json!([12.0, 17.0]), "{f:?}");
        assert_eq!((f[PRESET].clone(), f[PRESET_USER].clone()), (json!("Bright"), json!(true)));

        let mut f = fields(&mut app);
        run_preset_action(&mut app, &mut f, id, photocraft_doc::ColorMode::Rgb, PresetAction::Save);
        app.poll_file_dialog(&ctx, None);
        let (path, bytes) = written.borrow().first().cloned().unwrap();
        assert_eq!(path, "/presets/Mine.acv");
        let curves = photocraft_psd::acv::parse(&bytes).unwrap();
        assert_eq!(curves[0], vec![[0, 0], [12, 17], [50, 58], [128, 132], [255, 255]]);
        let names: Vec<_> = app.session.presets.curves.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["Bright", "Mine"]);
        assert_eq!(fields(&mut app)[PRESET], json!("Mine"));

        let mut f = fields(&mut app);
        run_preset_action(&mut app, &mut f, id, photocraft_doc::ColorMode::Rgb, PresetAction::Delete("Mine".into()));
        assert_eq!((f[PRESET].clone(), f[PRESET_USER].clone()), (json!(CUSTOM), json!(false)));
        assert_eq!(app.session.presets.curves.len(), 1);
        assert!(!app.ui.status_error, "{}", app.ui.status);
    }

    /// The gear lists the three preset actions; Delete Current Preset is greyed out unless a
    /// user preset is selected.
    #[test]
    fn delete_is_only_enabled_for_a_user_preset() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::{NodeT, Queryable};
        for user in [false, true] {
            let mut session = photocraft_engine::Session::new();
            session.execute("file.new", json!({"width": 16, "height": 16})).unwrap();
            session.execute("curves.presets.save", json!({"name": "Mine", "curves": [[[0, 0], [128, 160], [255, 255]]]})).unwrap();
            let app = PhotocraftApp::new(session, crate::Services::default());
            let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_ui_state(|ui, app| crate::dialogs::show(app, ui.ctx()), app);
            PhotocraftApp::setup_context(&h.ctx, Default::default());
            let id = crate::adjust_dialog::open(h.state_mut(), "image.adjustments.curves").unwrap();
            if user {
                set_preset(&mut h.state_mut().ui.dialog_mut(id).unwrap().fields, "Mine", true);
            }
            h.run_steps(3);
            h.get_by_label("Preset Options").click();
            h.run_steps(3);
            assert!(h.query_by_label("Save Preset…").is_some() && h.query_by_label("Load Preset…").is_some());
            assert_eq!(h.get_by_label("Delete Current Preset").accesskit_node().is_disabled(), !user);
        }
    }

    #[test]
    fn pigment_flips_are_their_own_inverse() {
        let pts = vec![[0.0, 10.0], [100.0, 160.0], [255.0, 240.0]];
        let shown = flip_points(&pts, true);
        assert_eq!(shown, vec![[0.0, 15.0], [155.0, 95.0], [255.0, 245.0]], "sorted by shown input");
        assert_eq!(flip_points(&shown, true), pts);
        let map: Vec<f32> = (0..256).map(|k| (k as f32 * 0.5).round()).collect();
        assert_eq!(flip_map(&flip_map(&map, true), true), map);
        assert_eq!(flip_map(&map, false), map);
        assert_eq!(flip_map(&[], true), Vec::<f32>::new(), "no panic on an empty map");
    }

    #[test]
    fn auto_stretches_each_channel_and_leaves_the_composite() {
        let mut bins = [0u32; 256];
        bins[20] = 500;
        bins[200] = 500;
        let h = Histograms { tag: 0, ch: vec![bins; 4] };
        let mut v = json!({"points": [[0, 0], [128, 140], [255, 255]]});
        assert!(auto_curves(&mut v, &h, false, None));
        assert_eq!(v["red"], json!([[20.0, 0.0], [200.0, 255.0]]));
        assert_eq!(v["blue"], json!([[20.0, 0.0], [200.0, 255.0]]));
        assert_eq!(v["points"], json!([[0, 0], [128, 140], [255, 255]]));
        let mut g = json!({});
        assert!(auto_curves(&mut g, &h, true, None));
        assert_eq!(g["points"], json!([[20.0, 0.0], [200.0, 255.0]]));
        // Empty histograms are no panic.
        let mut e = json!({});
        assert!(auto_curves(&mut e, &Histograms::default(), false, None));
    }
}
