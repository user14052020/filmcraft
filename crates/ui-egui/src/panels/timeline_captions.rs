//! Caption track rows of the Timeline: drawn in their own area above the video tracks (Premiere's
//! layout). Each row has a header (C1…, name, format, output eye, lock) and caption blocks in a
//! distinct colour showing their text. Click selects (Cmd/Shift adds), dragging the body moves,
//! dragging an edge trims, double-click opens the Text panel's Captions tab. Every gesture ends in
//! one `captions.*` or mixed `timeline.move` command; every element registers an automation id (`timeline.caption.*`,
//! `timeline.captionTrack.C1.*`).

use egui::{Align2, Color32, CursorIcon, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{CaptionTrack, ClipId, Sequence, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use super::timeline::{Drag, Layout};
use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::icons::Icon;
use crate::theme::Tokens;

/// Height of one caption track row.
pub const ROW_H: f32 = 28.0;
/// Caption block colours (fill, selected fill).
const FILL: Color32 = Color32::from_rgb(0x6b, 0x4f, 0x8f);
const FILL_SEL: Color32 = Color32::from_rgb(0x8c, 0x6c, 0xb4);

/// Row rects (full width incl. header) for each caption track, top first.
pub fn rows(area: Rect, n: usize) -> Vec<Rect> {
    (0..n).map(|i| Rect::from_min_size(pos2(area.min.x, area.min.y + i as f32 * ROW_H), vec2(area.width(), ROW_H))).collect()
}

#[derive(Clone, Copy, Debug)]
enum Gesture {
    Move,
    TrimIn,
    TrimOut,
}

fn drag_id() -> egui::Id {
    egui::Id::new("caption-drag")
}

/// Live drag state: (caption, gesture, delta ticks).
fn drag_state(ctx: &egui::Context) -> Option<(u64, u8, i64)> {
    ctx.data(|d| d.get_temp::<(u64, u8, i64)>(drag_id()))
}

fn move_offset(app: &FilmcraftApp, id: ClipId) -> Option<Tick> {
    match &app.tl.drag {
        Some(Drag::Move { captions, offset, .. }) if captions.contains(&id) => Some(*offset),
        _ => None,
    }
}

fn block_rect(c: &filmcraft_project::Caption, row: Rect, layout: &Layout, drag: Option<(u64, u8, i64)>, offset: Option<Tick>) -> Rect {
    let (mut s, mut e) = (c.start, c.end());
    if let Some(delta) = offset {
        s += delta;
        e += delta;
    } else if let Some((id, g, d)) = drag.filter(|d| d.0 == c.id.0) {
        let _ = id;
        match g {
            0 => {
                s += Tick(d);
                e += Tick(d);
            }
            1 => s = (s + Tick(d)).min(e),
            _ => e = (e + Tick(d)).max(s),
        }
    }
    Rect::from_min_max(pos2(layout.x_of(s), row.min.y + 2.0), pos2(layout.x_of(e).max(layout.x_of(s) + 1.0), row.max.y - 2.0))
}

/// Paint headers and caption blocks.
pub fn paint(app: &mut FilmcraftApp, ui: &mut egui::Ui, seq: &Sequence, area: Rect, layout: &Layout, t: &Tokens) {
    if seq.caption_tracks.is_empty() {
        return;
    }
    let hw = app.ui.timeline.header_w;
    let content = layout.content;
    let painter = ui.painter().with_clip_rect(area);
    let sel = app.session.state.caption_selection.clone();
    let drag = drag_state(ui.ctx());
    let mut actions: Vec<(String, Value)> = Vec::new();
    for (i, (tr, row)) in seq.caption_tracks.iter().zip(rows(area, seq.caption_tracks.len())).enumerate() {
        let lane = Rect::from_min_max(pos2(content.min.x, row.min.y), pos2(content.max.x, row.max.y));
        painter.rect_filled(lane, 0.0, t.caption_track_bg);
        painter.line_segment([pos2(row.min.x, row.max.y - 0.5), pos2(row.max.x, row.max.y - 0.5)], Stroke::new(1.0, t.tl_bg));
        // header
        let label = format!("C{}", i + 1);
        let hrect = Rect::from_min_max(row.min, pos2(row.min.x + hw, row.max.y));
        painter.rect_filled(hrect, 0.0, t.tl_header_bg);
        painter.line_segment([pos2(hrect.min.x, hrect.max.y - 0.5), pos2(hrect.max.x, hrect.max.y - 0.5)], Stroke::new(1.0, t.separator));
        let badge = Rect::from_min_size(pos2(hrect.min.x + 13.0, row.min.y + 3.0), vec2(24.0, ROW_H - 6.0));
        painter.rect_filled(badge, 3.0, if tr.enabled { FILL } else { t.field_bg });
        painter.text(badge.center(), Align2::CENTER_CENTER, &label, Tokens::semibold(10.5), Color32::WHITE);
        app.auto.add(&format!("timeline.captionTrack.{label}"), badge, &tr.name);
        // right-click the header: delete this track or every empty caption track (registered
        // before the lock / eye toggles so those stay on top)
        let hresp = ui.interact(hrect, egui::Id::new(("cap-header", tr.id.0)), Sense::click());
        hresp.context_menu(|ui| {
            for (key, text, cmd, p) in [
                ("delete", tl!("Delete Caption Track"), "captions.deleteTrack", json!({"track": tr.id.0})),
                ("deleteEmpty", tl!("Delete Empty Caption Tracks"), "sequence.deleteTracks", json!({"captions": "empty"})),
            ] {
                let enabled = key != "deleteEmpty" || seq.caption_tracks.iter().any(|t| t.captions.is_empty());
                let r = ui.add_enabled(enabled, egui::Button::new(text));
                app.auto.add(&format!("timeline.captionTrack.{label}.menu.{key}"), r.rect, text);
                if r.clicked() {
                    actions.push((cmd.into(), p));
                    ui.close();
                }
            }
        });
        let lock_r = Rect::from_center_size(pos2(hrect.min.x + 49.0, row.center().y), vec2(18.0, 18.0));
        let lresp = crate::widgets::icon_toggle(
            ui,
            lock_r,
            if tr.locked { Icon::Lock } else { Icon::Unlock },
            true,
            t,
            egui::Id::new(("cap-lock", tr.id.0)),
            Some(if tr.locked { t.icon_active } else { t.text_dim }),
        );
        app.auto.add(&format!("timeline.captionTrack.{label}.locked"), lock_r, "Toggle Caption Track Lock");
        if lresp.clicked() {
            actions.push(("captions.setTrack".into(), json!({"track": tr.id.0, "locked": !tr.locked})));
        }
        let eye_r = Rect::from_center_size(pos2(hrect.min.x + 73.0, row.center().y), vec2(18.0, 18.0));
        let eresp = crate::widgets::icon_toggle(
            ui,
            eye_r,
            if tr.enabled { Icon::Eye } else { Icon::EyeOff },
            true,
            t,
            egui::Id::new(("cap-eye", tr.id.0)),
            Some(t.text_dim),
        );
        app.auto.add(&format!("timeline.captionTrack.{label}.enabled"), eye_r, "Toggle Caption Track Output");
        if eresp.clicked() {
            actions.push(("captions.setTrack".into(), json!({"track": tr.id.0, "enabled": !tr.enabled})));
        }
        let name = format!("{} · {}", tr.name, crate::i18n::t(tr.format.label()));
        painter.with_clip_rect(Rect::from_min_max(pos2(hrect.min.x + 86.0, row.min.y), pos2(hrect.max.x - 4.0, row.max.y))).text(
            pos2(hrect.min.x + 90.0, row.center().y),
            Align2::LEFT_CENTER,
            name,
            Tokens::ui(11.0),
            t.text_dim,
        );
        // caption blocks
        let p = painter.with_clip_rect(lane);
        let copying = matches!(app.tl.drag, Some(Drag::Move { copy: true, .. }));
        for c in &tr.captions {
            let offset = move_offset(app, c.id);
            let preview = block_rect(c, row, layout, drag, offset);
            let original = (copying && offset.is_some()).then(|| block_rect(c, row, layout, None, None));
            for r in original.into_iter().chain(std::iter::once(preview)).filter(|r| r.intersects(lane)) {
                let selected = sel.contains(&c.id);
                p.rect_filled(r, 3.0, if selected { FILL_SEL } else { FILL });
                if selected {
                    p.rect_stroke(r, 3.0, Stroke::new(1.0, t.clip_selected_border), StrokeKind::Inside);
                }
                if r.width() > 14.0 {
                    let text = filmcraft_project::plain_text(&c.text).replace('\n', " ");
                    crate::rtl::paint_line(
                        &p.with_clip_rect(r.shrink2(vec2(4.0, 0.0)).intersect(lane)),
                        Rect::from_min_max(pos2(r.min.x + 5.0, r.min.y), pos2(r.max.x - 5.0, r.max.y)),
                        &text, Tokens::ui(11.0), if tr.enabled { Color32::WHITE } else { t.text_dim },
                    );
                }
                app.auto.add(&format!("timeline.caption.{}", c.id.0), r.intersect(lane), &c.text);
            }
        }
        if tr.locked {
            p.rect_filled(lane, 0.0, Color32::from_black_alpha(70));
        }
    }
    // column separator
    painter.line_segment([pos2(area.min.x + hw - 0.5, area.min.y), pos2(area.min.x + hw - 0.5, area.max.y)], Stroke::new(1.0, t.separator));
    run(app, ui, actions);
}

/// Pointer interaction on caption blocks (call after the timeline's own interaction so captions
/// are on top).
pub fn interact(app: &mut FilmcraftApp, ui: &mut egui::Ui, seq: &Sequence, area: Rect, layout: &Layout) {
    let ctx = ui.ctx().clone();
    let drag = drag_state(&ctx);
    if drag.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape) || (!i.pointer.any_down() && !i.pointer.any_released())) {
        if drag.is_some_and(|(_, g, _)| g == 0) {
            app.tl.drag = None;
        }
        ctx.data_mut(|d| d.remove::<(u64, u8, i64)>(drag_id()));
        return;
    }
    let mut actions: Vec<(String, Value)> = Vec::new();
    for (tr, row) in seq.caption_tracks.iter().zip(rows(area, seq.caption_tracks.len())) {
        let lane = Rect::from_min_max(pos2(layout.content.min.x, row.min.y), pos2(layout.content.max.x, row.max.y));
        for c in &tr.captions {
            let mut r = block_rect(c, row, layout, drag, move_offset(app, c.id)).intersect(lane);
            if !r.is_positive() {
                if drag.is_none_or(|(id, _, _)| id != c.id.0) {
                    continue;
                }
                // Keep the active widget alive when the drag leaves the viewport, so release
                // and Escape still reach it. The pointer remains captured by the same id.
                r = Rect::from_min_size(lane.min, vec2(1.0, ROW_H));
            }
            interact_block(app, ui, seq, tr, c, r, layout, &mut actions);
        }
    }
    run(app, ui, actions);
}

fn interact_block(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    seq: &Sequence,
    tr: &CaptionTrack,
    c: &filmcraft_project::Caption,
    r: Rect,
    layout: &Layout,
    actions: &mut Vec<(String, Value)>,
) {
    let ctx = ui.ctx().clone();
    let rate = seq.settings.frame_rate;
    let mods = ctx.input(|i| i.modifiers);
    let resp = ui.interact(r, egui::Id::new(("caption-block", c.id.0)), Sense::click_and_drag());
    let edge = 5.0f32.min(r.width() / 4.0);
    let gesture_at = |x: f32| {
        if x - r.min.x < edge { Gesture::TrimIn }
        else if r.max.x - x < edge { Gesture::TrimOut }
        else { Gesture::Move }
    };
    if let Some(p) = resp.hover_pos() && !tr.locked {
        ctx.set_cursor_icon(match gesture_at(p.x) {
            Gesture::Move => CursorIcon::Default,
            _ => CursorIcon::ResizeColumn,
        });
    }
    if resp.double_clicked() {
        app.show_panel(PanelKind::Text);
        app.ui.text_tab = "Captions".into();
        actions.push(("captions.goTo".into(), json!({"caption": c.id.0})));
        return;
    }
    if resp.clicked() {
        let add = mods.command || mods.shift;
        if add || !app.session.state.caption_selection.contains(&c.id) {
            actions.push(("captions.select".into(), json!({"captions": [c.id.0], "add": add})));
        }
    }
    if tr.locked {
        return;
    }
    if resp.drag_started() && let Some(p) = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()) {
        let g = match gesture_at(p.x) {
            Gesture::Move => 0u8,
            Gesture::TrimIn => 1,
            Gesture::TrimOut => 2,
        };
        if !app.session.state.caption_selection.contains(&c.id) {
            if let Err(e) = app.session.execute("captions.select", json!({"captions": [c.id.0], "add": mods.shift || mods.command})) {
                app.ui.status = e.to_string();
                return;
            }
        }
        let grab_tick = layout.tick_at(p.x);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("caption-drag-tick"), grab_tick.0));
        ctx.data_mut(|d| d.insert_temp(drag_id(), (c.id.0, g, 0i64)));
        if g == 0 {
            let clips = app.session.state.selection.clone();
            let start_track = clips.iter().find_map(|id| seq.find_item(*id).map(|(tid, _)| tid)).unwrap_or(TrackId(0));
            app.tl.drag = Some(Drag::Move {
                clips, captions: app.session.state.caption_selection.clone(), grab_tick, start_track,
                offset: Tick::ZERO, track_delta: 0, new_track: None, copy: mods.alt,
            });
        }
    }
    if resp.dragged()
        && let (Some((id, g, _)), Some(p)) = (drag_state(&ctx), resp.interact_pointer_pos())
        && id == c.id.0
    {
        let origin = Tick(ctx.data(|d| d.get_temp::<i64>(egui::Id::new("caption-drag-tick"))).unwrap_or_else(|| layout.tick_at(p.x).0));
        let mut delta = rate.snap_nearest(layout.tick_at(p.x) - origin);
        if g == 0 {
            if let Some(Drag::Move { clips, captions, offset, copy, .. }) = &mut app.tl.drag {
                delta = match filmcraft_edit::captions::selection_move_delta(seq, clips, captions, delta, !mods.command && !mods.alt) {
                    Ok(d) => d,
                    Err(e) => { app.ui.status = e.to_string(); Tick::ZERO }
                };
                *offset = delta;
                *copy = mods.alt;
            }
        }
        ctx.data_mut(|d| d.insert_temp(drag_id(), (id, g, delta.0)));
        let v = &mut app.ui.timeline;
        if v.pps.is_finite() && v.pps > 0.0 {
            if p.x > layout.content.max.x - 20.0 {
                v.target_scroll += 6.0 / v.pps;
                v.scroll = v.target_scroll;
            } else if p.x < layout.content.min.x + 10.0 && v.scroll > 0.0 {
                v.target_scroll = (v.target_scroll - 6.0 / v.pps).max(0.0);
                v.scroll = v.target_scroll;
            }
        }
    }
    if resp.drag_stopped() && let Some((id, g, d)) = drag_state(&ctx) && id == c.id.0 {
        ctx.data_mut(|dd| dd.remove::<(u64, u8, i64)>(drag_id()));
        let cancelled = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if g == 0 {
            if let Some(Drag::Move { clips, captions, offset, copy, .. }) = app.tl.drag.take()
                && offset != Tick::ZERO && !cancelled
            {
                let moves: Vec<Value> = clips.iter().filter_map(|id| seq.find_item(*id)
                    .map(|(tid, it)| json!({"clip": id.0, "track": tid.0, "time": (it.start + offset).0}))).collect();
                actions.push(("timeline.move".into(), json!({"moves": moves,
                    "captions": captions.iter().map(|c| c.0).collect::<Vec<_>>(), "delta": offset.0,
                    "insert": mods.command, "linked": false, "copy": copy || mods.alt})));
            }
        } else if d != 0 && !cancelled {
            actions.push(("captions.trim".into(), json!({"caption": id, "edge": if g == 1 { "in" } else { "out" }, "delta": d})));
        }
    }
}

fn run(app: &mut FilmcraftApp, ui: &egui::Ui, actions: Vec<(String, Value)>) {
    let ctx = ui.ctx().clone();
    for (cmd, p) in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &cmd, p) {
            app.ui.status = e;
        }
    }
}
