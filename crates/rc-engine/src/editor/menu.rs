//! RAC Maker's main menu (docs/plan/rac_maker.md, Phase 4 round): the maps in the maps folder as cards with their
//! thumbnails (taken in the editor with Set thumbnail: the free camera's view, no panels), the one picked with its
//! named versions and automatic backups, and the usual buttons (continue, new, load, play, duplicate, rename, delete,
//! the maps folder, the guide, quit). It opens when RAC Maker starts and from the top bar's Menu.

use super::*;
use std::time::SystemTime;

/// A map in the maps folder, read for its card.
pub(super) struct MapCard {
    pub path: PathBuf,
    name: String,
    modified: SystemTime,
    /// Objects, islands, paths, rails; None: the file does not read.
    counts: Option<(usize, usize, usize, usize)>,
}

#[derive(Default)]
pub(super) struct MainMenu {
    pub open: bool,
    cards: Option<Vec<MapCard>>,
    selected: Option<PathBuf>,
    rename: String,
    new_name: String,
    version_name: String,
    confirm_delete: bool,
    /// The card being dragged to a new place (its index in `cards`).
    dragging: Option<usize>,
    /// Per thumbnail file: when it was written and its texture.
    thumbs: HashMap<PathBuf, (SystemTime, Option<egui::TextureId>)>,
}

impl MainMenu {
    /// Open (RAC Maker starts on it).
    pub fn opened() -> MainMenu { MainMenu { open: true, ..Default::default() } }
    /// Open again: the maps read afresh.
    pub fn reopen(&mut self) {
        self.open = true;
        self.cards = None;
        self.confirm_delete = false;
    }
}

/// A map's thumbnail file: its name with .png, beside it.
pub(super) fn thumb_path(map: &std::path::Path) -> PathBuf { map.with_extension("png") }

/// The folder of a map's named versions (not pruned, unlike the backups).
fn versions_dir(map: &std::path::Path) -> PathBuf {
    maps_dir().join("versions").join(map.file_stem().map_or("map".into(), |s| s.to_string_lossy().into_owned()))
}

/// A map's named versions, newest first: (file, label, when).
fn versions(map: &std::path::Path) -> Vec<(PathBuf, String)> {
    let mut v: Vec<(PathBuf, String)> = std::fs::read_dir(versions_dir(map))
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let n = p.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned());
            // "<stamp> <name>"
            let (t, name) = n.split_once(' ').unwrap_or((n.as_str(), ""));
            let when = if t.len() == 15 { format!("{}-{}-{} {}:{}", &t[0..4], &t[4..6], &t[6..8], &t[9..11], &t[11..13]) } else { t.to_string() };
            let label = if name.is_empty() { when } else { format!("{name}  ({when})") };
            (p, label)
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v
}

/// How long ago, in words.
fn ago(t: SystemTime) -> String {
    let s = SystemTime::now().duration_since(t).map_or(0, |d| d.as_secs());
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

/// The maps folder read again: the maps in the order the cards were dragged into (`order`: file names), the maps not
/// in it first, newest first.
fn read_cards(order: &[String]) -> Vec<MapCard> {
    let mut v: Vec<MapCard> = std::fs::read_dir(maps_dir())
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|path| {
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let def = std::fs::read_to_string(&path).ok().and_then(|t| MapDef::parse(&t).ok());
            let name = def.as_ref().map(|d| d.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| path.file_stem().map_or("?".into(), |s| s.to_string_lossy().into_owned()));
            let counts = def.map(|d| (d.objects.len(), d.patches.len(), d.walkways.len(), d.rails.len()));
            MapCard { path, name, modified, counts }
        })
        .collect();
    v.sort_by(|a, b| b.modified.cmp(&a.modified));
    let place = |c: &MapCard| c.path.file_name().and_then(|f| order.iter().position(|o| std::ffi::OsStr::new(o) == f));
    v.sort_by_key(|c| place(c).map_or(0, |k| k + 1));
    v
}

/// A map file that does not exist yet in the maps folder: `name`, else `name 2`, `name 3`...
fn free_path(name: &str) -> PathBuf {
    let clean: String = name.chars().map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let clean = clean.trim();
    let base = if clean.is_empty() { "Map" } else { clean };
    let dir = maps_dir();
    std::iter::once(dir.join(format!("{base}.json"))).chain((2..).map(|n| dir.join(format!("{base} {n}.json")))).find(|p| !p.exists()).unwrap()
}

impl MainMenu {
    /// A thumbnail's texture (loaded again when the file changes).
    fn thumb(&mut self, contexts: &mut EguiContexts, images: &mut Assets<Image>, map: &std::path::Path) -> Option<egui::TextureId> {
        let file = thumb_path(map);
        let when = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
        if let Some((t, id)) = self.thumbs.get(&file) {
            if *t == when { return *id; }
        }
        let id = std::fs::read(&file).ok().and_then(|b| {
            Image::from_buffer(&b, ImageType::Extension("png"), CompressedImageFormats::NONE, true, ImageSampler::linear(), RenderAssetUsages::RENDER_WORLD).ok()
        }).map(|img| contexts.add_image(EguiTextureHandle::Strong(images.add(img))));
        self.thumbs.insert(file, (when, id));
        id
    }
}

/// The menu over the whole window; returns whether it is still open.
pub(super) fn main_menu(ctx: &egui::Context, ed: &mut Editor, contexts: &mut EguiContexts, images: &mut Assets<Image>, exit: &mut MessageWriter<AppExit>) {
    let orange = egui::Color32::from_rgb(0xf0, 0x8a, 0x24);
    let screen = ctx.viewport_rect();
    if ed.menu.cards.is_none() {
        ed.menu.cards = Some(read_cards(&ed.settings.order));
        if ed.menu.selected.is_none() { ed.menu.selected = Some(ed.path.clone()); }
    }
    let mut action: Option<Box<dyn FnOnce(&mut Editor)>> = None;
    egui::Area::new(egui::Id::new("rac_maker_menu")).fixed_pos(screen.min).order(egui::Order::Foreground).show(ctx, |ui| {
        ui.set_min_size(screen.size());
        // The world shows dimmed behind it.
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_rgba_unmultiplied(8, 12, 26, 228));
        let margin = 28.0;
        let area = screen.shrink(margin);
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("RAC MAKER").size(44.0).strong().color(orange));
                ui.add_space(12.0);
                ui.label(egui::RichText::new("make your own Ratchet & Clank levels").size(18.0).weak());
            });
            ui.add_space(14.0);
            // The columns share the width (a small window keeps them all on screen).
            let left_w = (area.width() * 0.2).clamp(190.0, 260.0);
            let right_w = (area.width() * 0.3).clamp(260.0, 420.0);
            ui.horizontal_top(|ui| {
                // ---------------------------------------------------------------- the buttons
                ui.vertical(|ui| {
                    ui.set_width(left_w);
                    let big = |ui: &mut egui::Ui, text: &str, fill: Option<egui::Color32>| {
                        let mut b = egui::Button::new(egui::RichText::new(text).size(19.0).strong()).min_size(egui::vec2(left_w, 46.0));
                        if let Some(f) = fill { b = b.fill(f); }
                        ui.add(b)
                    };
                    let current = ed.path.file_stem().map_or("map".into(), |s| s.to_string_lossy().into_owned());
                    if big(ui, &format!("▶ Continue: {current}"), Some(orange)).on_hover_text("Back to the map you are editing").clicked() {
                        ed.menu.open = false;
                    }
                    ui.add_space(6.0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(left_w - 14.0);
                        ui.label(egui::RichText::new("New map").strong().size(16.0));
                        ui.add(egui::TextEdit::singleline(&mut ed.menu.new_name).hint_text("Its name").desired_width(left_w - 30.0));
                        if ui.add(egui::Button::new(egui::RichText::new("Create").strong()).min_size(egui::vec2(left_w - 30.0, 30.0))).clicked() {
                            let name = if ed.menu.new_name.trim().is_empty() { "Map".to_string() } else { ed.menu.new_name.trim().to_string() };
                            action = Some(Box::new(move |ed: &mut Editor| {
                                let path = free_path(&name);
                                if !ed.saved { ed.backup(&ed.def.to_json(), true); }
                                let def = MapDef::new_default(&path.file_stem().map_or(name.clone(), |s| s.to_string_lossy().into_owned()), ed.def.host_level);
                                ed.switch_to(path, def, false);
                                ed.save();
                                ed.menu.open = false;
                                ed.menu.new_name.clear();
                            }));
                        }
                    });
                    ui.add_space(6.0);
                    if big(ui, "Open maps folder", None).clicked() {
                        let _ = std::fs::create_dir_all(maps_dir());
                        let _ = open_in_shell(&maps_dir());
                    }
                    if big(ui, "Guide", None).on_hover_text("How everything works, in your browser").clicked() { let _ = open_guide(); }
                    if big(ui, "Help in the editor", None).clicked() {
                        ed.help = Some(0);
                        ed.menu.open = false;
                    }
                    ui.add_space(18.0);
                    if big(ui, "Quit", None).clicked() {
                        if !ed.saved { ed.backup(&ed.def.to_json(), true); }
                        exit.write(AppExit::Success);
                    }
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Unsaved work is kept as a backup when you leave.").weak().small());
                });
                ui.add_space(18.0);
                // ---------------------------------------------------------------- the maps
                let mid_w = (area.width() - left_w - right_w - 60.0).max(300.0);
                ui.vertical(|ui| {
                    ui.set_width(mid_w);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Your maps").size(22.0).strong());
                        ui.label(egui::RichText::new("drag a card to move it").weak().small());
                        if ui.small_button("Refresh").clicked() { ed.menu.cards = None; }
                    });
                    ui.add_space(6.0);
                    let mut cards = ed.menu.cards.take().unwrap_or_default();
                    let mut rects: Vec<egui::Rect> = Vec::new();
                    // As many columns as fit, the cards as wide as that leaves.
                    let per_row = ((mid_w - 20.0) / 250.0).floor().max(1.0) as usize;
                    let card_w = ((mid_w - 20.0) / per_row as f32 - 30.0).clamp(160.0, 320.0);
                    egui::ScrollArea::vertical().id_salt("menu_maps").max_height(area.height() - 110.0).auto_shrink([false; 2]).show(ui, |ui| {
                        for (row_n, row) in cards.chunks(per_row).enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(14.0, 14.0);
                            for (col, c) in row.iter().enumerate() {
                                let index = row_n * per_row + col;
                                let dragged = ed.menu.dragging == Some(index);
                                let tex = ed.menu.thumb(contexts, images, &c.path);
                                let on = ed.menu.selected.as_ref() == Some(&c.path);
                                let open_now = c.path == ed.path;
                                let r = egui::Frame::group(ui.style())
                                    .fill(if dragged { egui::Color32::from_rgb(0x10, 0x16, 0x26) } else if on { egui::Color32::from_rgb(0x3a, 0x2a, 0x14) } else { egui::Color32::from_rgb(0x1c, 0x26, 0x3e) })
                                    .stroke(egui::Stroke::new(if on { 2.0 } else { 1.0 }, if on { orange } else { egui::Color32::from_rgb(0x34, 0x44, 0x66) }))
                                    .show(ui, |ui| ui.vertical(|ui| {
                                        ui.set_width(card_w);
                                        let size = egui::vec2(card_w, card_w * 9.0 / 16.0);
                                        match tex {
                                            Some(t) => { ui.add(egui::Image::new((t, size))); }
                                            None => {
                                                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                                                ui.painter().rect_filled(rect, 4.0, egui::Color32::from_rgb(0x24, 0x34, 0x58));
                                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, c.name.chars().next().unwrap_or('?').to_uppercase().to_string(), egui::FontId::proportional(56.0), egui::Color32::from_rgb(0x5a, 0x74, 0xa8));
                                            }
                                        }
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(&c.name).size(17.0).strong());
                                            if open_now { ui.label(egui::RichText::new("open").small().color(orange)); }
                                        });
                                        let counts = match c.counts {
                                            Some((o, i, p, r)) => format!("{o} objects · {i} islands · {p} paths · {r} rails"),
                                            None => "does not read".into(),
                                        };
                                        ui.label(egui::RichText::new(format!("edited {}", ago(c.modified))).weak().small());
                                        ui.label(egui::RichText::new(counts).weak().small());
                                    }))
                                    .response;
                                rects.push(r.rect);
                                let r = ui.interact(r.rect, egui::Id::new(("menu_card", &c.path)), egui::Sense::click_and_drag());
                                if r.drag_started() { ed.menu.dragging = Some(index); }
                                if r.clicked() {
                                    ed.menu.selected = Some(c.path.clone());
                                    ed.menu.rename = c.name.clone();
                                    ed.menu.confirm_delete = false;
                                }
                                if r.double_clicked() {
                                    let path = c.path.clone();
                                    action = Some(Box::new(move |ed: &mut Editor| {
                                        if path != ed.path { ed.open(path); }
                                        ed.menu.open = false;
                                    }));
                                }
                            }
                        });
                        ui.add_space(10.0);
                        }
                        if cards.is_empty() { ui.label(egui::RichText::new("No maps yet: make one with New map.").weak()); }
                    });
                    // A card being dragged: an orange bar where it would go, its name at the pointer; let go to put it there.
                    if let (Some(from), Some(at)) = (ed.menu.dragging, ui.ctx().pointer_latest_pos()) {
                        let to = rects.iter().enumerate().min_by(|a, b| a.1.center().distance(at).total_cmp(&b.1.center().distance(at))).map(|(k, r)| if at.x > r.center().x { k + 1 } else { k });
                        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("menu_drag")));
                        if let Some(to) = to {
                            let bar = if to < rects.len() { rects[to].left() - 7.0 } else { rects[rects.len() - 1].right() + 7.0 };
                            let r = rects[to.min(rects.len() - 1)];
                            painter.line_segment([egui::pos2(bar, r.top()), egui::pos2(bar, r.bottom())], egui::Stroke::new(4.0, orange));
                        }
                        if let Some(c) = cards.get(from) {
                            let text = egui::RichText::new(&c.name).size(17.0).strong().color(orange);
                            let galley = egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::FontSelection::Default);
                            let rect = egui::Rect::from_min_size(at + egui::vec2(14.0, 8.0), galley.size() + egui::vec2(16.0, 10.0));
                            painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(0x1c, 0x26, 0x3e));
                            painter.galley(rect.min + egui::vec2(8.0, 5.0), galley, orange);
                        }
                        if !ui.ctx().input(|i| i.pointer.any_down()) {
                            ed.menu.dragging = None;
                            if let Some(to) = to.filter(|&to| from < cards.len() && to != from && to != from + 1) {
                                let c = cards.remove(from);
                                cards.insert(if to > from { to - 1 } else { to }, c);
                                ed.settings.order = cards.iter().filter_map(|c| c.path.file_name().map(|f| f.to_string_lossy().into_owned())).collect();
                                ed.settings.store();
                            }
                        }
                    }
                    ed.menu.cards = Some(cards);
                });
                ui.add_space(18.0);
                // ---------------------------------------------------------------- the map picked
                ui.vertical(|ui| {
                    ui.set_width(right_w);
                    let Some(sel) = ed.menu.selected.clone() else { return };
                    let Some(card) = ed.menu.cards.as_ref().and_then(|c| c.iter().position(|m| m.path == sel)) else {
                        ui.label(egui::RichText::new("Pick a map.").weak());
                        return;
                    };
                    let (name, counts, modified) = {
                        let c = &ed.menu.cards.as_ref().unwrap()[card];
                        (c.name.clone(), c.counts, c.modified)
                    };
                    egui::Frame::group(ui.style()).fill(egui::Color32::from_rgb(0x16, 0x1f, 0x34)).show(ui, |ui| {
                        ui.set_width(right_w - 14.0);
                        let size = egui::vec2(right_w - 14.0, (right_w - 14.0) * 9.0 / 16.0);
                        match ed.menu.thumb(contexts, images, &sel) {
                            Some(t) => { ui.add(egui::Image::new((t, size))); }
                            None => {
                                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                                ui.painter().rect_filled(rect, 4.0, egui::Color32::from_rgb(0x24, 0x34, 0x58));
                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "No thumbnail yet\nopen the map, fly the camera to a nice view\nand press Set thumbnail (top bar)", egui::FontId::proportional(15.0), egui::Color32::from_rgb(0x9a, 0xaa, 0xcc));
                            }
                        }
                        ui.label(egui::RichText::new(&name).size(24.0).strong());
                        ui.label(egui::RichText::new(format!("edited {}", ago(modified))).weak());
                        if let Some((o, i, p, r)) = counts { ui.label(egui::RichText::new(format!("{o} objects, {i} islands, {p} paths, {r} rails")).weak()); }
                        ui.add_space(8.0);
                        let open_now = sel == ed.path;
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(egui::RichText::new(if open_now { "Continue" } else { "Load" }).size(18.0).strong()).fill(orange).min_size(egui::vec2(110.0, 38.0))).clicked() {
                                let path = sel.clone();
                                action = Some(Box::new(move |ed: &mut Editor| {
                                    if path != ed.path { ed.open(path); }
                                    ed.menu.open = false;
                                }));
                            }
                            if ui.add(egui::Button::new(egui::RichText::new("▶ Play").size(18.0).strong()).min_size(egui::vec2(100.0, 38.0))).on_hover_text("Load it and play from its start (F5 stops)").clicked() {
                                let path = sel.clone();
                                action = Some(Box::new(move |ed: &mut Editor| {
                                    if path != ed.path { ed.open(path); }
                                    ed.play_pending = true;
                                    ed.menu.open = false;
                                }));
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Duplicate").clicked() {
                                let (src, name) = (sel.clone(), name.clone());
                                action = Some(Box::new(move |ed: &mut Editor| {
                                    let to = free_path(&format!("{name} copy"));
                                    let ok = std::fs::read_to_string(&src).ok().and_then(|t| MapDef::parse(&t).ok()).map(|mut d| {
                                        d.name = to.file_stem().map_or(d.name.clone(), |s| s.to_string_lossy().into_owned());
                                        std::fs::write(&to, d.to_json()).is_ok()
                                    });
                                    if ok == Some(true) {
                                        let _ = std::fs::copy(thumb_path(&src), thumb_path(&to));
                                        ed.menu.selected = Some(to);
                                        ed.status = "Map duplicated".into();
                                    }
                                    ed.menu.cards = None;
                                }));
                            }
                            if !ed.menu.confirm_delete {
                                if ui.add_enabled(!open_now, egui::Button::new("Delete")).on_disabled_hover_text("It is the map open now: load another first").on_hover_text("Moves it to the Deleted folder in the maps folder").clicked() {
                                    ed.menu.confirm_delete = true;
                                }
                            } else {
                                ui.label(egui::RichText::new("Delete it?").color(egui::Color32::from_rgb(0xff, 0x80, 0x60)));
                                if ui.button("Yes, delete").clicked() {
                                    let src = sel.clone();
                                    action = Some(Box::new(move |ed: &mut Editor| {
                                        let bin = maps_dir().join("Deleted");
                                        let _ = std::fs::create_dir_all(&bin);
                                        let stem = src.file_stem().map_or("map".into(), |s| s.to_string_lossy().into_owned());
                                        let tag = stamp(now_secs()).1;
                                        let _ = std::fs::rename(&src, bin.join(format!("{stem} {tag}.json")));
                                        let _ = std::fs::rename(thumb_path(&src), bin.join(format!("{stem} {tag}.png")));
                                        ed.menu.selected = Some(ed.path.clone());
                                        ed.menu.confirm_delete = false;
                                        ed.menu.cards = None;
                                        ed.status = format!("{stem} moved to the Deleted folder");
                                    }));
                                }
                                if ui.button("No").clicked() { ed.menu.confirm_delete = false; }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut ed.menu.rename).desired_width(right_w - 110.0));
                            if ui.button("Rename").clicked() && !ed.menu.rename.trim().is_empty() && ed.menu.rename.trim() != name {
                                let (src, new) = (sel.clone(), ed.menu.rename.trim().to_string());
                                action = Some(Box::new(move |ed: &mut Editor| {
                                    let to = free_path(&new);
                                    let renamed = std::fs::read_to_string(&src).ok().and_then(|t| MapDef::parse(&t).ok()).map(|mut d| {
                                        d.name = new.clone();
                                        std::fs::write(&to, d.to_json()).is_ok() && std::fs::remove_file(&src).is_ok()
                                    });
                                    if renamed != Some(true) {
                                        ed.status = "Could not rename it".into();
                                        return;
                                    }
                                    let _ = std::fs::rename(thumb_path(&src), thumb_path(&to));
                                    let _ = std::fs::rename(backup_dir(&src), backup_dir(&to));
                                    let _ = std::fs::rename(versions_dir(&src), versions_dir(&to));
                                    if src == ed.path {
                                        ed.path = to.clone();
                                        ed.def.name = new.clone();
                                        ed.settings.opened(&to);
                                    }
                                    ed.menu.selected = Some(to);
                                    ed.menu.cards = None;
                                    ed.status = format!("Renamed to {new}");
                                }));
                            }
                        });
                    });
                    ui.add_space(8.0);
                    // Versions and backups.
                    egui::ScrollArea::vertical().id_salt("menu_versions").max_height((area.height() - 560.0).max(160.0)).show(ui, |ui| {
                        ui.label(egui::RichText::new("Versions").strong().size(17.0)).on_hover_text("Copies of the map you keep on purpose, by name");
                        if sel == ed.path {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut ed.menu.version_name).hint_text("e.g. before the boss arena").desired_width(right_w - 150.0));
                                if ui.button("Save version").on_hover_text("Keeps the map as it is now (unsaved changes too)").clicked() {
                                    let dir = versions_dir(&ed.path);
                                    let label: String = ed.menu.version_name.trim().chars().map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
                                    let file = dir.join(format!("{} {}.json", stamp(now_secs()).1, if label.is_empty() { "version" } else { &label }));
                                    ed.status = match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, ed.def.to_json())) {
                                        Ok(()) => format!("Version saved: {}", file.file_stem().map_or(String::new(), |s| s.to_string_lossy().into_owned())),
                                        Err(e) => format!("Version not saved: {e}"),
                                    };
                                    ed.menu.version_name.clear();
                                }
                            });
                        } else {
                            ui.label(egui::RichText::new("Load the map to save a version of it.").weak().small());
                        }
                        let vs = versions(&sel);
                        if vs.is_empty() { ui.label(egui::RichText::new("None yet.").weak().small()); }
                        for (file, label) in vs {
                            ui.horizontal(|ui| {
                                ui.label(label);
                                if ui.small_button("Load").on_hover_text("Opens the map as it was then (save to keep it)").clicked() {
                                    let path = sel.clone();
                                    action = Some(Box::new(move |ed: &mut Editor| load_copy(ed, path, file)));
                                }
                            });
                        }
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Backups").strong().size(17.0)).on_hover_text("Kept by themselves: before every save, every 5 minutes of unsaved work, and when you leave");
                        let bs = backups(&sel);
                        if bs.is_empty() { ui.label(egui::RichText::new("None yet.").weak().small()); }
                        for (file, label) in bs.into_iter().take(30) {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(label).small());
                                if ui.small_button("Load").on_hover_text("Opens the map as it was then (save to keep it)").clicked() {
                                    let path = sel.clone();
                                    action = Some(Box::new(move |ed: &mut Editor| load_copy(ed, path, file)));
                                }
                            });
                        }
                    });
                });
            });
        });
    });
    if let Some(a) = action { a(ed); }
}

/// Opens map `path` as it is in `copy` (a version or a backup): unsaved, so saving brings it back.
fn load_copy(ed: &mut Editor, path: PathBuf, copy: PathBuf) {
    let Ok(def) = std::fs::read_to_string(&copy).map_err(|e| e.to_string()).and_then(|t| MapDef::parse(&t).map_err(|e| e.to_string())) else {
        ed.status = format!("Could not read {}", copy.display());
        return;
    };
    if !ed.saved { ed.backup(&ed.def.to_json(), true); }
    ed.switch_to(path, def, false);
    ed.status = "Opened an earlier copy: Ctrl+S makes it the map again".into();
    ed.menu.open = false;
}
