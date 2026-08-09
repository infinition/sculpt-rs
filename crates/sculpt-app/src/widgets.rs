//! Touch-first widgets.
//!
//! egui's stock widgets are sized for a mouse. These wrap the same interaction
//! model in bigger targets, draw the icon set from [`crate::icons`], and put the
//! value inside the control so a fingertip never covers the number it is
//! setting.

use crate::icons::{self, Icon};
use crate::theme::Palette;
use egui::{
    Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, Ui, Vec2,
};

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
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    paint_icon_button(ui, rect, &response, icon, selected, false);

    let icon_rect = Rect::from_min_size(
        rect.left_top() + Vec2::new(height * 0.18, 0.0),
        Vec2::new(height, height),
    );
    let color = content_color(&response, selected);
    icons::paint(ui.painter(), icons::centered_rect(icon_rect, height * 0.56), icon, color);
    ui.painter().text(
        egui::pos2(icon_rect.right() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(ui.style().text_styles[&egui::TextStyle::Button].size),
        color,
    );
    response
}

/// A tool tile: icon on top, name underneath, used by the brush rail.
pub fn tool_tile(ui: &mut Ui, icon: Icon, name: &str, size: f32, selected: bool, tip: &str) -> Response {
    // The label needs its own band, otherwise the highlight of a selected tile
    // crops the descenders.
    let label_band = 16.0;
    let height = size + label_band;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(size, height), Sense::click());
    paint_icon_button(ui, rect, &response, icon, selected, false);

    let color = content_color(&response, selected);
    let glyph = Rect::from_min_size(rect.left_top(), Vec2::new(size, size));
    icons::paint(ui.painter(), icons::centered_rect(glyph, size * 0.6), icon, color);
    ui.painter().text(
        egui::pos2(rect.center().x, rect.bottom() - label_band * 0.5),
        Align2::CENTER_CENTER,
        name,
        FontId::proportional(9.5),
        if selected { color } else { Palette::DIM },
    );
    response.on_hover_text(tip)
}

fn content_color(response: &Response, selected: bool) -> Color32 {
    if selected {
        Palette::ACCENT
    } else if response.hovered() {
        Palette::TEXT
    } else {
        Palette::DIM
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
    let radius = CornerRadius::same(10);
    let fill = if selected {
        Palette::ACCENT_DIM
    } else if response.is_pointer_button_down_on() {
        Palette::HOVER
    } else if response.hovered() {
        Palette::RAISED
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if selected {
        Stroke::new(1.0, Palette::ACCENT)
    } else {
        Stroke::NONE
    };
    ui.painter().rect(rect, radius, fill, stroke, egui::StrokeKind::Inside);
    if draw_glyph {
        let color = content_color(response, selected);
        icons::paint(
            ui.painter(),
            icons::centered_rect(rect, rect.width() * 0.56),
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
        let width = ui.available_width();
        let (rect, mut response) =
            ui.allocate_exact_size(Vec2::new(width, self.height), Sense::click_and_drag());

        if response.is_pointer_button_down_on() {
            if let Some(p) = ui.ctx().pointer_interact_pos() {
                let t = (p.x - rect.left()) / rect.width().max(1e-3);
                let next = self.from_normalised(t);
                if (next - *self.value).abs() > f32::EPSILON {
                    *self.value = next;
                    response.mark_changed();
                }
            }
        }

        let t = self.to_normalised(*self.value);
        let radius = CornerRadius::same(9);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, Palette::BG);

        let mut fill = rect;
        fill.set_right(rect.left() + rect.width() * t);
        if fill.width() > 2.0 {
            painter.rect_filled(fill, radius, Palette::ACCENT_DIM);
        }
        if response.hovered() || response.dragged() {
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, Palette::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        // Handle, wide enough to see under a finger.
        let hx = rect.left() + rect.width() * t;
        painter.rect_filled(
            Rect::from_center_size(egui::pos2(hx, rect.center().y), Vec2::new(3.0, rect.height() - 8.0)),
            CornerRadius::same(2),
            Palette::ACCENT,
        );

        let font = FontId::proportional(ui.style().text_styles[&egui::TextStyle::Small].size);
        painter.text(
            egui::pos2(rect.left() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            self.label,
            font.clone(),
            Palette::TEXT,
        );
        painter.text(
            egui::pos2(rect.right() - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            format!("{:.*}{}", self.decimals, *self.value, self.suffix),
            font,
            Palette::DIM,
        );
        response
    }
}

/// Pill toggle with a label.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str, height: f32) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    if response.clicked() {
        *on = !*on;
    }
    let painter = ui.painter();
    let radius = CornerRadius::same(9);
    let bg = if response.hovered() { Palette::HOVER } else { Palette::RAISED };
    painter.rect_filled(rect, radius, bg);

    let knob_w = height * 1.7;
    let track = Rect::from_min_size(
        egui::pos2(rect.right() - knob_w - 8.0, rect.center().y - height * 0.28),
        Vec2::new(knob_w, height * 0.56),
    );
    painter.rect_filled(
        track,
        CornerRadius::same((track.height() * 0.5) as u8),
        if *on { Palette::ACCENT } else { Palette::LINE },
    );
    let r = track.height() * 0.5 - 2.0;
    let cx = if *on { track.right() - r - 2.0 } else { track.left() + r + 2.0 };
    painter.circle_filled(egui::pos2(cx, track.center().y), r, Palette::TEXT);

    painter.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(ui.style().text_styles[&egui::TextStyle::Small].size),
        Palette::TEXT,
    );
    response
}

/// Segmented control. Returns the index that was clicked, if any.
pub fn segmented(ui: &mut Ui, labels: &[&str], current: usize, height: f32) -> Option<usize> {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(9), Palette::BG);

    let n = labels.len().max(1);
    let seg = rect.width() / n as f32;
    let mut clicked = None;
    let font = FontId::proportional(ui.style().text_styles[&egui::TextStyle::Small].size);
    for (i, label) in labels.iter().enumerate() {
        let cell = Rect::from_min_size(
            egui::pos2(rect.left() + seg * i as f32, rect.top()),
            Vec2::new(seg, rect.height()),
        );
        let id = ui.id().with(("segmented", i, *label));
        let r = ui.interact(cell, id, Sense::click());
        if r.clicked() {
            clicked = Some(i);
        }
        let active = i == current;
        if active {
            ui.painter()
                .rect_filled(cell.shrink(3.0), CornerRadius::same(7), Palette::ACCENT_DIM);
        } else if r.hovered() {
            ui.painter()
                .rect_filled(cell.shrink(3.0), CornerRadius::same(7), Palette::RAISED);
        }
        ui.painter().text(
            cell.center(),
            Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if active { Palette::ACCENT } else { Palette::DIM },
        );
    }
    clicked
}

/// Small caps section title with a hairline.
pub fn section_title(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(10.0),
        Palette::FAINT,
    );
    let text_w = text.len() as f32 * 6.0 + 10.0;
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + text_w, rect.center().y),
            egui::pos2(rect.right(), rect.center().y),
        ],
        Stroke::new(1.0, Palette::LINE),
    );
}

/// Colour row: a wide swatch that opens egui's picker, with the hex beside it.
///
/// egui's own colour button is a fixed 20 points square, which is unusable with
/// a fingertip, so the swatch here is full width and the real button rides on
/// top of it.
pub fn color_row(ui: &mut Ui, rgb: &mut [f32; 3], height: f32) -> Response {
    let color = Color32::from_rgb(
        (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
    );
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(9),
        color,
        Stroke::new(1.0, Palette::LINE),
        egui::StrokeKind::Inside,
    );
    let luma = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
    ui.painter().text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        format!(
            "#{:02X}{:02X}{:02X}",
            (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
            (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
            (rgb[2].clamp(0.0, 1.0) * 255.0) as u8
        ),
        FontId::monospace(11.0),
        if luma > 0.55 { Color32::from_rgb(20, 20, 20) } else { Palette::TEXT },
    );
    let button = Rect::from_center_size(
        egui::pos2(rect.right() - height * 0.5 - 6.0, rect.center().y),
        Vec2::splat(height - 10.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(button), |ui| {
        ui.color_edit_button_rgb(rgb)
    })
    .inner
}
