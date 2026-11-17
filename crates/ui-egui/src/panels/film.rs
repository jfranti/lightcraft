//! The Film panel: render the photo as a film stock — the scene-referred negative, print, grain and
//! sharpness of `lightcraft_pipeline::film` (the photo's `film` settings) — and film LUT looks (seeded
//! looks and stock-inspired LUT profiles, `profile.generate`).

use egui::{Align2, Sense, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::Film;
use lightcraft_pipeline::film::{Kind, PRINTS, STOCKS, default_print, stock};
use serde_json::json;

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::{divider, register};

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    let d = (*app.session.develop_of(id).unwrap_or_default()).clone();
    let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
    ui.painter().text(pos2(hr.left() + 24.0, hr.center().y + 2.0), Align2::LEFT_CENTER, crate::i18n::tr("Film"), t.semibold(15.0), t.text);
    divider(ui);
    let mut f = d.film.clone();
    let last_id = egui::Id::new("film-last-stock");
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 10, bottom: 12 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let mut on = f.is_on();
        let toggle = ui.checkbox(&mut on, crate::i18n::tr("Render as film"));
        register(ui.ctx(), "filmOn", toggle.rect);
        if toggle.changed() {
            f.stock = if on { ui.data(|m| m.get_temp::<String>(last_id)).unwrap_or_else(|| "vision3-250d".into()) } else { String::new() };
        }
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "The film's negative, print, grain and sharpness take over from the camera's tone curve. Fine detail is lost as on real film.",
            ))
            .color(t.text_dim)
            .size(11.5),
        );
        let Some(s) = stock(&f.stock) else {
            apply(app, &d.film, &f);
            return;
        };
        ui.data_mut(|m| m.insert_temp(last_id, f.stock.clone()));
        // manufacturer, then stock
        // each maker once, in the stock list's order
        let mut makers: Vec<&str> = Vec::new();
        for x in STOCKS {
            if !makers.contains(&x.maker) {
                makers.push(x.maker);
            }
        }
        let mut maker = s.maker;
        row(ui, &t, "Maker", |ui| {
            let r = egui::ComboBox::from_id_salt("film-maker").width(180.0).selected_text(maker).show_ui(ui, |ui| {
                for m in &makers {
                    ui.selectable_value(&mut maker, m, *m);
                }
            });
            register(ui.ctx(), "filmMaker", r.response.rect);
        });
        if maker != s.maker
            && let Some(first) = STOCKS.iter().find(|x| x.maker == maker)
        {
            f.stock = first.id.into();
        }
        row(ui, &t, "Stock", |ui| {
            let current = stock(&f.stock).map_or("", |x| x.name);
            let r = egui::ComboBox::from_id_salt("film-stock-pick").width(180.0).selected_text(current).show_ui(ui, |ui| {
                for x in STOCKS.iter().filter(|x| x.maker == maker) {
                    ui.selectable_value(&mut f.stock, x.id.to_string(), x.name);
                }
            });
            register(ui.ctx(), "filmStockPick", r.response.rect);
        });
        // the stock's characteristics
        if let Some(s) = stock(&f.stock) {
            let kind = match s.kind {
                Kind::Negative if s.mono.is_some() => crate::i18n::tr("Black-and-white negative"),
                Kind::Negative => crate::i18n::tr("Colour negative"),
                Kind::Reversal => crate::i18n::tr("Colour reversal (slide)"),
            };
            let balance = if s.balance == 'T' { crate::i18n::tr("Tungsten (3200 K)") } else { crate::i18n::tr("Daylight") };
            egui::Frame::NONE.fill(t.canvas).corner_radius(4.0).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(format!("{kind} · ISO {} · {balance}", s.iso)).color(t.text_label).size(12.0));
                ui.label(egui::RichText::new(s.notes).color(t.text_dim).size(11.5));
            });
        }
        // format
        row(ui, &t, "Format", |ui| {
            for fmt in ["16mm", "35mm", "120"] {
                let b = ui.selectable_label(f.format == fmt, fmt);
                register(ui.ctx(), format!("filmFormat:{fmt}"), b.rect);
                if b.clicked() {
                    f.format = fmt.into();
                }
            }
        });
        // print (negatives)
        if let Some(s) = stock(&f.stock)
            && s.kind == Kind::Negative
            && s.mono.is_none()
        {
            let usual = default_print(s);
            row(ui, &t, "Print", |ui| {
                let name = |id: &str| PRINTS.iter().find(|p| p.id == id).map_or("", |p| p.name);
                let shown = if f.print.is_empty() { format!("{} ({})", name(usual), crate::i18n::tr("usual")) } else { name(&f.print).to_string() };
                let r = egui::ComboBox::from_id_salt("film-print").width(180.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.print, String::new(), format!("{} ({})", name(usual), crate::i18n::tr("usual")));
                    for p in PRINTS {
                        ui.selectable_value(&mut f.print, p.id.to_string(), p.name);
                    }
                });
                register(ui.ctx(), "filmPrint", r.response.rect);
            });
        }
        // vibrance
        row(ui, &t, "Colour", |ui| {
            for (v, label) in [(0.0, "Base"), (1.0, "Vibrant"), (2.0, "Very vibrant")] {
                let b = ui.selectable_label((f.vibrance - v).abs() < 0.01, crate::i18n::tr(label));
                register(ui.ctx(), format!("filmVibrance:{label}"), b.rect);
                if b.clicked() {
                    f.vibrance = v;
                }
            }
        });
        // grain: applied when the drag ends (one undo step)
        row(ui, &t, "Grain", |ui| {
            let gid = egui::Id::new(("film-grain-drag", id.0));
            let mut g: f64 = ui.data(|m| m.get_temp(gid)).unwrap_or(f.grain);
            let r = ui.add(egui::Slider::new(&mut g, 0.0..=100.0).fixed_decimals(0));
            register(ui.ctx(), "filmGrain", r.rect);
            if r.dragged() {
                ui.data_mut(|m| m.insert_temp(gid, g));
            } else {
                ui.data_mut(|m| m.remove::<f64>(gid));
                if r.changed() || r.drag_stopped() {
                    f.grain = g;
                }
            }
        });
        // halation: applied when the drag ends (one undo step)
        row(ui, &t, "Halation", |ui| {
            let hid = egui::Id::new(("film-halation-drag", id.0));
            let mut h: f64 = ui.data(|m| m.get_temp(hid)).unwrap_or(f.halation);
            let r = ui.add(egui::Slider::new(&mut h, 0.0..=100.0).fixed_decimals(0)).on_hover_text(crate::i18n::tr(
                "The red-orange glow around the brightest light, coming back off the film base (50: the stock's own)",
            ));
            register(ui.ctx(), "filmHalation", r.rect);
            if r.dragged() {
                ui.data_mut(|m| m.insert_temp(hid, h));
            } else {
                ui.data_mut(|m| m.remove::<f64>(hid));
                if r.changed() || r.drag_stopped() {
                    f.halation = h;
                }
            }
        });
        let hc = ui.checkbox(&mut f.highlight_compression, crate::i18n::tr("Highlight compression"));
        register(ui.ctx(), "filmHighlightCompression", hc.rect);
        let ds = ui.checkbox(&mut f.desaturate_ends, crate::i18n::tr("Desaturate shadows & highlights"));
        register(ui.ctx(), "filmDesaturate", ds.rect);
        apply(app, &d.film, &f);
    });
    divider(ui);
    film_generator(app, ui);
}

/// A labelled row.
fn row(ui: &mut egui::Ui, t: &Tokens, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(56.0, 20.0), egui::Label::new(egui::RichText::new(crate::i18n::tr(label)).color(t.text_dim).size(12.0)));
        add(ui);
    });
}

/// Write `new` film settings when they differ from `old` (one edit).
fn apply(app: &mut LightcraftApp, old: &Film, new: &Film) {
    if old != new
        && let Ok(v) = serde_json::to_value(new)
    {
        let _ = app.run("develop.merge", json!({"settings": {"film": v}, "label": "Film"}));
    }
}

/// The film look generator: a seed number, a dice button for a fresh one and Generate, which makes the
/// seed's film look (`profile.generate`, a profile in the Seeded Film group) and applies it. The same
/// seed always gives the same look, so a look can be shared as its number.
fn film_generator(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let seed_id = egui::Id::new("film-generator-seed");
    let mut seed: String = ui.data(|m| m.get_temp(seed_id)).unwrap_or_else(|| "1".to_string());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 8 }).show(ui, |ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("Film LUT looks")).color(t.text_label).size(13.0));
        ui.label(
            egui::RichText::new(crate::i18n::tr(
                "Creative looks as profiles (colour, grain and softness on top of the photo), from a seed or a stock.",
            ))
            .color(t.text_dim)
            .size(11.5),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(egui::RichText::new(crate::i18n::tr("Seed")).color(t.text_dim).size(12.0));
            let edit = ui.add(egui::TextEdit::singleline(&mut seed).desired_width(90.0).char_limit(19));
            register(ui.ctx(), "filmSeed", edit.rect);
            seed.retain(|c| c.is_ascii_digit());
            let dice = ui.button(crate::i18n::tr("Random")).on_hover_text(crate::i18n::tr("Pick a random seed and generate its look"));
            register(ui.ctx(), "filmSeedRandom", dice.rect);
            if dice.clicked() {
                let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64 % 1_000_000);
                seed = n.to_string();
            }
            let go = ui.button(crate::i18n::tr("Generate")).on_hover_text(crate::i18n::tr("Make this seed's film look and apply it"));
            register(ui.ctx(), "filmGenerate", go.rect);
            if (go.clicked() || dice.clicked() || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                && let Ok(n) = seed.parse::<u64>()
            {
                // the look's profile with its stock's grain and softness, as one edit
                match app.run("profile.generate", json!({"seed": n, "apply": true})) {
                    Ok(r) => {
                        let name = r["imported"].get(0).and_then(|p| p["name"].as_str()).unwrap_or_default().to_string();
                        app.toast(ui.ctx(), format!("{}: {name}", crate::i18n::tr("Film look applied")));
                    }
                    Err(e) => app.toast(ui.ctx(), e.to_string()),
                }
            }
        });
        // film stocks: a named stock's look, grain and softness
        ui.add_space(4.0);
        let stock_id = egui::Id::new("film-generator-stock");
        let stocks = lightcraft_pipeline::film_seed::STOCKS;
        let mut pick: usize = ui.data(|m| m.get_temp(stock_id)).unwrap_or(1).min(stocks.len() - 1);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(egui::RichText::new(crate::i18n::tr("Stock")).color(t.text_dim).size(12.0));
            let combo = egui::ComboBox::from_id_salt("film-stock").width(170.0).show_index(ui, &mut pick, stocks.len(), |i| stocks[i].1.to_string());
            register(ui.ctx(), "filmStock", combo.rect);
            let apply = ui.button(crate::i18n::tr("Apply")).on_hover_text(crate::i18n::tr("Apply this film stock's look, grain and softness"));
            register(ui.ctx(), "filmStockApply", apply.rect);
            if apply.clicked() {
                match app.run("profile.generate", json!({"stock": stocks[pick].0, "apply": true})) {
                    Ok(_) => app.toast(ui.ctx(), format!("{}: {}", crate::i18n::tr("Film look applied"), stocks[pick].1)),
                    Err(e) => app.toast(ui.ctx(), e.to_string()),
                }
            }
        });
        ui.data_mut(|m| m.insert_temp(stock_id, pick));
    });
    ui.data_mut(|m| m.insert_temp(seed_id, seed));
}
