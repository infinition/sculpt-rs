//! Touch-first widgets.
//!
//! egui's stock widgets are sized for a mouse. These wrap the same interaction
//! model in bigger targets, draw the icon set from [`crate::icons`], and put the
//! value inside the control so a fingertip never covers the number it is
//! setting. Colours come from the palette the theme installed this frame.

use crate::icons::{self, Icon};
use crate::theme::{Metrics, Palette};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};

/// Square icon button. `selected` paints it in the accent colour.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, selected: bool, tip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    paint_icon_button(ui, rect, &response, icon, selected, true);
    if !tip.is_empty() {
        return response.on_hover_text(tip);
    }
    response
}

/// Icon button that fills the width it is given, with a label beside the icon.
pub fn wide_button(ui: &mut Ui, icon: Icon, text: &str, height: f32, selected: bool) -> Response {
    let p = Palette::ui(ui);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    paint_icon_button(ui, rect, &response, icon, selected, false);

    let icon_rect = Rect::from_min_size(
        rect.left_top() + Vec2::new(height * 0.18, 0.0),
        Vec2::new(height, height),
    );
    let color = content_color(&p, &response, selected);
    icons::paint(
        ui.painter(),
        icons::centered_rect(icon_rect, height * 0.56 * icon_scale(ui)),
        icon,
        color,
    );
    // Clip the label so a narrow dock truncates rather than overflows.
    let text_rect = Rect::from_min_max(
        egui::pos2(icon_rect.right() + 6.0, rect.top()),
        rect.right_bottom(),
    );
    let painter = ui.painter().with_clip_rect(text_rect.intersect(ui.clip_rect()));
    painter.text(
        egui::pos2(text_rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size),
        color,
    );
    response
}

/// A tool tile: icon on top, name underneath, used by the brush rail.
pub fn tool_tile(
    ui: &mut Ui,
    icon: Icon,
    name: &str,
    size: f32,
    selected: bool,
    tip: &str,
) -> Response {
    let p = Palette::ui(ui);
    // The label needs its own band, otherwise the highlight of a selected tile
    // crops the descenders.
    let label_band = (size * 0.3).clamp(13.0, 22.0);
    let height = size + label_band;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(size, height), Sense::click());
    paint_icon_button(ui, rect, &response, icon, selected, false);

    let color = content_color(&p, &response, selected);
    let glyph = Rect::from_min_size(rect.left_top(), Vec2::new(size, size));
    icons::paint(
        ui.painter(),
        icons::centered_rect(glyph, size * 0.6 * icon_scale(ui)),
        icon,
        color,
    );
    ui.painter().text(
        egui::pos2(rect.center().x, rect.bottom() - label_band * 0.5),
        Align2::CENTER_CENTER,
        name,
        FontId::proportional((size * 0.21).clamp(8.0, 15.0)),
        if selected { color } else { p.dim },
    );
    response.on_hover_text(tip)
}

/// Compact tile with no label, for a rail docked along the top or bottom.
pub fn tool_chip(ui: &mut Ui, icon: Icon, size: f32, selected: bool, tip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    paint_icon_button(ui, rect, &response, icon, selected, true);
    response.on_hover_text(tip)
}

fn icon_scale(ui: &Ui) -> f32 {
    ui.ctx()
        .data(|d| d.get_temp::<f32>(egui::Id::new("sculpt_icon_scale")))
        .unwrap_or(1.0)
}

/// Stores the glyph size multiplier for the frame.
pub fn set_icon_scale(ctx: &egui::Context, scale: f32) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("sculpt_icon_scale"), scale));
}

fn content_color(p: &Palette, response: &Response, selected: bool) -> Color32 {
    if selected {
        p.accent
    } else if response.hovered() {
        p.text
    } else {
        p.dim
    }
}

fn paint_icon_button(
    ui: &Ui,
    rect: Rect,
    response: &Response,
    icon: Icon,
    selected: bool,
    draw_glyph: bool,
) {
    let p = Palette::ui(ui);
    let radius = CornerRadius::same(ui.visuals().widgets.inactive.corner_radius.nw);
    let fill = if selected {
        p.accent_dim
    } else if response.is_pointer_button_down_on() {
        p.hover
    } else if response.hovered() {
        p.raised
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if selected {
        Stroke::new(1.0, p.accent)
    } else {
        Stroke::NONE
    };
    ui.painter()
        .rect(rect, radius, fill, stroke, egui::StrokeKind::Inside);
    if draw_glyph {
        let color = content_color(&p, response, selected);
        icons::paint(
            ui.painter(),
            icons::centered_rect(rect, rect.width() * 0.56 * icon_scale(ui)),
            icon,
            color,
        );
    }
}

/// Horizontal slider with the label and value drawn inside the rail.
///
/// The whole row is draggable, which matters on a touch screen where hitting a
/// thin handle is hopeless.
pub struct BigSlider<'a> {
    value: &'a mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &'a str,
    logarithmic: bool,
    decimals: usize,
    suffix: &'a str,
    height: f32,
}

impl<'a> BigSlider<'a> {
    pub fn new(value: &'a mut f32, range: std::ops::RangeInclusive<f32>, label: &'a str) -> Self {
        Self {
            value,
            range,
            label,
            logarithmic: false,
            decimals: 2,
            suffix: "",
            height: 30.0,
        }
    }

    pub fn logarithmic(mut self, on: bool) -> Self {
        self.logarithmic = on;
        self
    }

    pub fn decimals(mut self, n: usize) -> Self {
        self.decimals = n;
        self
    }

    pub fn suffix(mut self, s: &'a str) -> Self {
        self.suffix = s;
        self
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    fn to_normalised(&self, v: f32) -> f32 {
        let (lo, hi) = (*self.range.start(), *self.range.end());
        if self.logarithmic {
            let (lo, hi) = (lo.max(1e-6), hi.max(1e-6));
            ((v.max(1e-6) / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0)
        } else {
            ((v - lo) / (hi - lo).max(1e-9)).clamp(0.0, 1.0)
        }
    }

    fn from_normalised(&self, t: f32) -> f32 {
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let t = t.clamp(0.0, 1.0);
        if self.logarithmic {
            let (lo, hi) = (lo.max(1e-6), hi.max(1e-6));
            lo * (hi / lo).powf(t)
        } else {
            lo + (hi - lo) * t
        }
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let p = Palette::ui(ui);
        let width = ui.available_width();
        let (rect, mut response) =
            ui.allocate_exact_size(Vec2::new(width, self.height), Sense::click_and_drag());

        // Jump to where the press landed, then track the drag as a delta.
        //
        // Reading the absolute pointer position every frame looks equivalent
        // and is not: a slider that resizes the interface moves its own rail
        // out from under the cursor, and the next frame reads a position that
        // has nothing to do with the gesture. That feedback loop slams the
        // value to one end. A delta has no such loop.
        let mut t = self.to_normalised(*self.value);
        let width = rect.width().max(1e-3);
        let mut edited = false;

        if response.drag_started() || (response.clicked() && !response.dragged()) {
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                t = ((pos.x - rect.left()) / width).clamp(0.0, 1.0);
                edited = true;
            }
        }
        if response.dragged() {
            let delta = response.drag_delta().x;
            if delta != 0.0 {
                t = (t + delta / width).clamp(0.0, 1.0);
                edited = true;
            }
        }
        if edited {
            let next = self.from_normalised(t);
            if (next - *self.value).abs() > f32::EPSILON {
                *self.value = next;
                response.mark_changed();
            }
        }

        let t = self.to_normalised(*self.value);
        let radius = CornerRadius::same(ui.visuals().widgets.inactive.corner_radius.nw);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, p.bg);

        let mut fill = rect;
        fill.set_right(rect.left() + rect.width() * t);
        if fill.width() > 2.0 {
            painter.rect_filled(fill, radius, p.accent_dim);
        }
        if response.hovered() || response.dragged() {
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }
        // Handle, wide enough to see under a finger.
        let hx = rect.left() + rect.width() * t;
        painter.rect_filled(
            Rect::from_center_size(
                egui::pos2(hx, rect.center().y),
                Vec2::new(3.0, rect.height() - 8.0),
            ),
            CornerRadius::same(2),
            p.accent,
        );

        let font = FontId::proportional(ui.style().text_styles[&egui::TextStyle::Small].size);
        let value = format!("{:.*}{}", self.decimals, *self.value, self.suffix);
        let value_galley = painter.layout_no_wrap(value, font.clone(), p.dim);
        // Give the label whatever the value does not need, then clip it.
        let label_rect = Rect::from_min_max(
            egui::pos2(rect.left() + 10.0, rect.top()),
            egui::pos2(rect.right() - value_galley.size().x - 16.0, rect.bottom()),
        );
        painter
            .with_clip_rect(label_rect.intersect(ui.clip_rect()))
            .text(
                egui::pos2(label_rect.left(), rect.center().y),
                Align2::LEFT_CENTER,
                self.label,
                font,
                p.text,
            );
        painter.galley(
            egui::pos2(
                rect.right() - 10.0 - value_galley.size().x,
                rect.center().y - value_galley.size().y * 0.5,
            ),
            value_galley,
            p.dim,
        );
        response
    }
}

/// Pill toggle with a label.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str, height: f32) -> Response {
    let p = Palette::ui(ui);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    if response.clicked() {
        *on = !*on;
    }
    let painter = ui.painter();
    let radius = CornerRadius::same(ui.visuals().widgets.inactive.corner_radius.nw);
    let bg = if response.hovered() { p.hover } else { p.raised };
    painter.rect_filled(rect, radius, bg);

    let knob_w = height * 1.7;
    let track = Rect::from_min_size(
        egui::pos2(rect.right() - knob_w - 8.0, rect.center().y - height * 0.28),
        Vec2::new(knob_w, height * 0.56),
    );
    painter.rect_filled(
        track,
        CornerRadius::same((track.height() * 0.5) as u8),
        if *on { p.accent } else { p.line },
    );
    let r = track.height() * 0.5 - 2.0;
    let cx = if *on { track.right() - r - 2.0 } else { track.left() + r + 2.0 };
    painter.circle_filled(egui::pos2(cx, track.center().y), r, p.text);

    let label_rect = Rect::from_min_max(
        egui::pos2(rect.left() + 10.0, rect.top()),
        egui::pos2(track.left() - 8.0, rect.bottom()),
    );
    painter
        .with_clip_rect(label_rect.intersect(ui.clip_rect()))
        .text(
            egui::pos2(label_rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(ui.style().text_styles[&egui::TextStyle::Small].size),
            p.text,
        );
    response
}

/// Segmented control. Returns the index that was clicked, if any.
///
/// Labels are laid out on one row when they fit and wrapped onto several when
/// they do not, so a narrow dock stays usable.
pub fn segmented(ui: &mut Ui, labels: &[&str], current: usize, height: f32) -> Option<usize> {
    let p = Palette::ui(ui);
    let font = FontId::proportional(ui.style().text_styles[&egui::TextStyle::Small].size);
    let width = ui.available_width();
    let n = labels.len().max(1);

    // How many fit per row, given the longest label.
    let widest = labels
        .iter()
        .map(|l| l.len() as f32 * font.size * 0.62 + 16.0)
        .fold(0.0f32, f32::max);
    let per_row = ((width / widest.max(24.0)).floor() as usize).clamp(1, n);
    let rows = n.div_ceil(per_row);

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width, height * rows as f32 + 2.0 * (rows - 1) as f32),
        Sense::hover(),
    );
    let radius = CornerRadius::same(ui.visuals().widgets.inactive.corner_radius.nw);
    ui.painter().rect_filled(rect, radius, p.bg);

    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let row = i / per_row;
        let col = i % per_row;
        let in_row = per_row.min(n - row * per_row);
        let seg = rect.width() / in_row as f32;
        let cell = Rect::from_min_size(
            egui::pos2(
                rect.left() + seg * col as f32,
                rect.top() + (height + 2.0) * row as f32,
            ),
            Vec2::new(seg, height),
        );
        let r = ui.interact(cell, ui.id().with(("segmented", i, *label)), Sense::click());
        if r.clicked() {
            clicked = Some(i);
        }
        let active = i == current;
        if active {
            ui.painter()
                .rect_filled(cell.shrink(3.0), radius, p.accent_dim);
        } else if r.hovered() {
            ui.painter().rect_filled(cell.shrink(3.0), radius, p.accent_soft);
        }
        ui.painter()
            .with_clip_rect(cell.intersect(ui.clip_rect()))
            .text(
                cell.center(),
                Align2::CENTER_CENTER,
                *label,
                font.clone(),
                if active { p.accent } else { p.dim },
            );
    }
    clicked
}

/// Small caps section title with a hairline.
pub fn section_title(ui: &mut Ui, text: &str) {
    let p = Palette::ui(ui);
    ui.add_space(4.0);
    let font = FontId::proportional(10.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, p.faint);
    let text_w = galley.size().x;
    ui.painter().galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
        galley,
        p.faint,
    );
    if rect.left() + text_w + 12.0 < rect.right() {
        ui.painter().line_segment(
            [
                egui::pos2(rect.left() + text_w + 8.0, rect.center().y),
                egui::pos2(rect.right(), rect.center().y),
            ],
            Stroke::new(1.0, p.line),
        );
    }
}

/// Colour row: a wide swatch that opens egui's picker, with the hex beside it.
///
/// egui's own colour button is a fixed 20 points square, which is unusable with
/// a fingertip, so the swatch here is full width and the real button rides on
/// top of it.
pub fn color_row(ui: &mut Ui, label: &str, rgb: &mut [f32; 3], height: f32) -> Response {
    let p = Palette::ui(ui);
    let color = Color32::from_rgb(
        (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
    );
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let radius = CornerRadius::same(ui.visuals().widgets.inactive.corner_radius.nw);
    ui.painter().rect_filled(rect, radius, p.raised);
    // Swatch on the left, label in the middle, picker button on the right.
    let swatch = Rect::from_min_size(
        rect.left_top() + Vec2::splat(4.0),
        Vec2::new(height * 1.4, height - 8.0),
    );
    ui.painter().rect(
        swatch,
        CornerRadius::same(6),
        color,
        Stroke::new(1.0, p.line),
        egui::StrokeKind::Inside,
    );
    let text_rect = Rect::from_min_max(
        egui::pos2(swatch.right() + 8.0, rect.top()),
        egui::pos2(rect.right() - height, rect.bottom()),
    );
    ui.painter()
        .with_clip_rect(text_rect.intersect(ui.clip_rect()))
        .text(
            egui::pos2(text_rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(ui.style().text_styles[&egui::TextStyle::Small].size),
            p.text,
        );
    let button = Rect::from_center_size(
        egui::pos2(rect.right() - height * 0.5 - 4.0, rect.center().y),
        Vec2::splat(height - 10.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(button), |ui| {
        ui.color_edit_button_rgb(rgb)
    })
    .inner
}

/// A slider bound to an integer setting.
pub fn int_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
    m: &Metrics,
) -> bool {
    let mut v = *value as f32;
    let changed = BigSlider::new(&mut v, *range.start() as f32..=*range.end() as f32, label)
        .decimals(0)
        .height(m.row)
        .show(ui)
        .changed();
    if changed {
        *value = v.round() as u32;
    }
    changed
}
