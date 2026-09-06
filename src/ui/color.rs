//! The colour control behind every colour row in the inspector.
//!
//! GTK's own `ColorDialogButton` opens the stock chooser, whose custom-colour
//! page is taller than the space it is given and gets clipped, so the wheel
//! arrives half off the dialog. This is a popover sized by its own contents
//! instead: quick colours, the wheel, the two sliders and the hex box are all
//! measured here, so the picker is exactly as big as what is in it.
//!
//! Wheel geometry follows the reference implementation: the angle is the hue
//! and the distance from the centre is the saturation.

use std::cell::{Cell, RefCell};
use std::f64::consts::{PI, TAU};
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::core::model::Rgba8;
use crate::i18n::t;

/// Side of the hue/saturation wheel. It is the widest thing in the popover,
/// so it sets the popover's width and every other row is laid out under it.
const WHEEL: i32 = 168;
const RADIUS: f64 = WHEEL as f64 / 2.0;
/// Side of one quick colour's patch, and of the patch on the button that
/// opens the popover.
const SWATCH: i32 = 18;
const PREVIEW: (i32, i32) = (28, 18);
const QUICK_COLUMNS: i32 = 8;

/// The quick colours: a row of hues over a row of neutrals and darks. Two
/// rows of eight at `SWATCH` come out the width of the wheel.
const QUICK: [Rgba8; 16] = [
    [255, 60, 60, 255],
    [255, 138, 0, 255],
    [255, 212, 0, 255],
    [60, 208, 112, 255],
    [0, 196, 204, 255],
    [60, 120, 255, 255],
    [138, 76, 255, 255],
    [255, 76, 192, 255],
    [255, 255, 255, 255],
    [200, 200, 200, 255],
    [128, 128, 128, 255],
    [64, 64, 64, 255],
    [0, 0, 0, 255],
    [122, 74, 30, 255],
    [10, 92, 58, 255],
    [20, 40, 90, 255],
];

/// A colour button and the popover it opens. Cloning shares one widget and
/// one colour, the way a `gtk` widget handle does.
#[derive(Clone)]
pub struct ColorPick {
    inner: Rc<Inner>,
}

type Listener = Rc<dyn Fn(Rgba8)>;
struct Inner {
    button: gtk::MenuButton,
    preview: gtk::DrawingArea,
    wheel: gtk::DrawingArea,
    value: gtk::Scale,
    opacity: gtk::Scale,
    hex: gtk::Entry,
    /// Hue in degrees with saturation and value in 0..1, stored rather than
    /// read back out of the RGB: dragging into the grey centre, or the value
    /// slider down to black, would otherwise throw the hue away.
    hsv: Cell<(f64, f64, f64)>,
    alpha: Cell<u8>,
    /// Set while the widgets are being written from the stored colour, so
    /// their handlers can tell that the change did not come from the user.
    syncing: Cell<bool>,
    listeners: RefCell<Vec<Listener>>,
}

impl ColorPick {
    pub fn new() -> Self {
        let inner = Rc::new(Inner {
            button: gtk::MenuButton::new(),
            preview: gtk::DrawingArea::new(),
            wheel: gtk::DrawingArea::new(),
            value: gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01),
            opacity: gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 255.0, 1.0),
            hex: gtk::Entry::new(),
            hsv: Cell::new((0.0, 0.0, 0.0)),
            alpha: Cell::new(255),
            syncing: Cell::new(false),
            listeners: RefCell::new(Vec::new()),
        });
        inner.build();
        Self { inner }
    }

    /// The widget to pack, in place of the colour button this replaces.
    pub fn widget(&self) -> &gtk::MenuButton {
        &self.inner.button
    }

    pub fn rgba(&self) -> Rgba8 {
        self.inner.rgba()
    }

    /// Shows the colour the model holds. Never reports back: the model is the
    /// caller here, and an echo would come round again as an edit.
    pub fn set_rgba(&self, colour: Rgba8) {
        self.inner.set_rgba(colour);
    }

    /// Runs for every colour the user picks, and for nothing else.
    pub fn connect_changed(&self, on_change: impl Fn(Rgba8) + 'static) {
        self.inner.listeners.borrow_mut().push(Rc::new(on_change));
    }
}

impl Default for ColorPick {
    fn default() -> Self {
        Self::new()
    }
}

impl Inner {
    fn rgba(&self) -> Rgba8 {
        let (hue, saturation, value) = self.hsv.get();
        let [r, g, b] = hsv_to_rgb(hue, saturation, value);
        [r, g, b, self.alpha.get()]
    }

    fn set_rgba(&self, colour: Rgba8) {
        if self.rgba() == colour {
            return;
        }
        let [r, g, b, alpha] = colour;
        self.hsv.set(rgb_to_hsv([r, g, b]));
        self.alpha.set(alpha);
        self.refresh();
    }

    /// The stored colour pushed out to every widget that shows it.
    fn refresh(&self) {
        let (_, _, value) = self.hsv.get();
        let text = hex_of(self.rgba());
        self.syncing.set(true);
        self.value.set_value(value);
        self.opacity.set_value(self.alpha.get() as f64);
        if self.hex.text() != text {
            self.hex.set_text(&text);
        }
        self.syncing.set(false);
        self.button.set_tooltip_text(Some(&text));
        self.wheel.queue_draw();
        self.preview.queue_draw();
    }

    /// A colour the user chose: stored, shown, then handed to the listeners.
    fn pick(&self, hsv: (f64, f64, f64), alpha: u8) {
        self.hsv.set(hsv);
        self.alpha.set(alpha);
        self.refresh();
        let colour = self.rgba();
        // Cloned out of the cell before the calls: a listener is free to set
        // the colour straight back, and that must not find the list borrowed.
        let listeners = self.listeners.borrow().clone();
        for listener in listeners {
            listener(colour);
        }
    }

    fn build(self: &Rc<Self>) {
        self.preview.set_content_width(PREVIEW.0);
        self.preview.set_content_height(PREVIEW.1);
        {
            let inner = Rc::clone(self);
            self.preview.set_draw_func(move |_, cr, w, h| {
                paint_swatch(cr, w as f64, h as f64, inner.rgba())
            });
        }
        self.button.set_child(Some(&self.preview));
        self.button.set_valign(gtk::Align::Center);
        self.button.set_focus_on_click(false);
        self.build_wheel();
        self.build_sliders();
        self.build_hex();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_top(6);
        content.set_margin_bottom(6);
        content.set_margin_start(6);
        content.set_margin_end(6);
        content.append(&self.quick_colors());
        content.append(&self.wheel);
        content.append(&slider_row(
            "display-brightness-symbolic",
            t("Brightness"),
            &self.value,
        ));
        content.append(&slider_row(
            "view-reveal-symbolic",
            // Translators: How see-through the colour is, 0 to 255.
            t("Opacity"),
            &self.opacity,
        ));
        content.append(&self.hex);

        let popover = gtk::Popover::new();
        popover.set_child(Some(&content));
        self.button.set_popover(Some(&popover));
        self.refresh();
    }

    /// Two rows of one-click colours, the size of an icon rather than of a
    /// button, so the grid reads as a palette instead of as a toolbar.
    fn quick_colors(self: &Rc<Self>) -> gtk::Grid {
        let grid = gtk::Grid::new();
        grid.set_row_spacing(2);
        grid.set_column_spacing(2);
        grid.set_halign(gtk::Align::Center);
        for (index, &colour) in QUICK.iter().enumerate() {
            let button = swatch_button(colour);
            let inner = Rc::clone(self);
            // Every quick colour is opaque, so taking one keeps whatever the
            // opacity slider is set to rather than resetting it.
            button.connect_clicked(move |_| {
                let [r, g, b, _] = colour;
                inner.pick(rgb_to_hsv([r, g, b]), inner.alpha.get());
            });
            let index = index as i32;
            grid.attach(&button, index % QUICK_COLUMNS, index / QUICK_COLUMNS, 1, 1);
        }
        grid
    }

    fn build_wheel(self: &Rc<Self>) {
        self.wheel.set_content_width(WHEEL);
        self.wheel.set_content_height(WHEEL);
        self.wheel.set_halign(gtk::Align::Center);
        {
            let inner = Rc::clone(self);
            self.wheel
                .set_draw_func(move |_, cr, _, _| inner.draw_wheel(cr));
        }
        // The drag reports offsets from where it began, so the start point is
        // kept to turn them back into positions on the wheel.
        let start = Rc::new(Cell::new((0.0, 0.0)));
        let drag = gtk::GestureDrag::new();
        {
            let (inner, start) = (Rc::clone(self), Rc::clone(&start));
            drag.connect_drag_begin(move |_, x, y| {
                start.set((x, y));
                inner.pick_at(x, y);
            });
        }
        {
            let (inner, start) = (Rc::clone(self), Rc::clone(&start));
            drag.connect_drag_update(move |_, dx, dy| {
                let (x, y) = start.get();
                inner.pick_at(x + dx, y + dy);
            });
        }
        self.wheel.add_controller(drag);
    }

    fn build_sliders(self: &Rc<Self>) {
        {
            let inner = Rc::clone(self);
            self.value.connect_value_changed(move |scale| {
                if inner.syncing.get() {
                    return;
                }
                let (hue, saturation, _) = inner.hsv.get();
                inner.pick((hue, saturation, scale.value()), inner.alpha.get());
            });
        }
        let inner = Rc::clone(self);
        self.opacity.connect_value_changed(move |scale| {
            if inner.syncing.get() {
                return;
            }
            inner.pick(inner.hsv.get(), scale.value().round() as u8);
        });
    }

    fn build_hex(self: &Rc<Self>) {
        self.hex.set_width_chars(9);
        self.hex.set_max_width_chars(9);
        self.hex.set_placeholder_text(Some("#RRGGBB"));
        let inner = Rc::clone(self);
        self.hex.connect_activate(move |entry| {
            let Some([r, g, b, alpha]) = parse_hex(&entry.text()) else {
                // Put the colour back rather than leave the box showing a
                // value nothing else in the picker agrees with.
                inner.refresh();
                return;
            };
            inner.pick(rgb_to_hsv([r, g, b]), alpha);
        });
    }

    fn pick_at(&self, x: f64, y: f64) {
        let (dx, dy) = (x - RADIUS, y - RADIUS);
        let (_, _, value) = self.hsv.get();
        self.pick(
            (hue_at(dx, dy), (dx.hypot(dy) / RADIUS).min(1.0), value),
            self.alpha.get(),
        );
    }

    fn draw_wheel(&self, cr: &cairo::Context) {
        let (hue, saturation, value) = self.hsv.get();
        let Ok(mut surface) = cairo::ImageSurface::create(cairo::Format::ARgb32, WHEEL, WHEEL)
        else {
            return;
        };
        let stride = surface.stride() as usize;
        {
            let Ok(mut pixels) = surface.data() else {
                return;
            };
            for y in 0..WHEEL as usize {
                for x in 0..WHEEL as usize {
                    let (dx, dy) = (x as f64 - RADIUS, y as f64 - RADIUS);
                    let distance = dx.hypot(dy);
                    if distance > RADIUS {
                        continue;
                    }
                    // Fade the outermost pixel, or the rim is a staircase.
                    let edge = (RADIUS - distance).min(1.0);
                    let [r, g, b] = hsv_to_rgb(hue_at(dx, dy), distance / RADIUS, value);
                    // ARGB32 is premultiplied, and byte order is BGRA here.
                    let at = y * stride + x * 4;
                    pixels[at] = (b as f64 * edge) as u8;
                    pixels[at + 1] = (g as f64 * edge) as u8;
                    pixels[at + 2] = (r as f64 * edge) as u8;
                    pixels[at + 3] = (255.0 * edge) as u8;
                }
            }
        }
        let _ = cr.set_source_surface(&surface, 0.0, 0.0);
        let _ = cr.paint();
        draw_cursor(cr, hue, saturation);
    }
}

/// Where the wheel puts a hue: straight up from the centre is cyan-ish, and
/// the ring runs the way the reference implementation's does.
fn hue_at(dx: f64, dy: f64) -> f64 {
    (dy.atan2(-dx) + PI) / TAU * 360.0
}

/// The ring marking the colour in hand. Two circles, light inside dark, so it
/// stays visible over both the pale centre and the saturated rim.
fn draw_cursor(cr: &cairo::Context, hue: f64, saturation: f64) {
    let angle = hue / 360.0 * TAU - PI;
    let distance = saturation * RADIUS;
    let (x, y) = (
        RADIUS - distance * angle.cos(),
        RADIUS + distance * angle.sin(),
    );
    cr.set_line_width(1.5);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.arc(x, y, 4.5, 0.0, TAU);
    let _ = cr.stroke();
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.6);
    cr.arc(x, y, 6.0, 0.0, TAU);
    let _ = cr.stroke();
}

fn swatch_button(colour: Rgba8) -> gtk::Button {
    let patch = gtk::DrawingArea::new();
    patch.set_content_width(SWATCH);
    patch.set_content_height(SWATCH);
    patch.set_draw_func(move |_, cr, w, h| paint_swatch(cr, w as f64, h as f64, colour));
    let button = gtk::Button::new();
    button.set_child(Some(&patch));
    button.add_css_class("color-swatch");
    button.set_focus_on_click(false);
    button.set_tooltip_text(Some(&hex_of(colour)));
    button
}

/// One flat patch of colour with a hairline round it, over a checkerboard
/// when the colour is see-through enough for the checkerboard to matter.
fn paint_swatch(cr: &cairo::Context, w: f64, h: f64, [r, g, b, a]: Rgba8) {
    if a < 255 {
        let square = 5.0;
        cr.set_source_rgb(0.85, 0.85, 0.85);
        cr.rectangle(0.0, 0.0, w, h);
        let _ = cr.fill();
        cr.set_source_rgb(0.6, 0.6, 0.6);
        let mut row = 0;
        while (row as f64) * square < h {
            let mut column = row % 2;
            while (column as f64) * square < w {
                cr.rectangle(column as f64 * square, row as f64 * square, square, square);
                column += 2;
            }
            row += 1;
        }
        let _ = cr.fill();
    }
    let byte = |v: u8| v as f64 / 255.0;
    cr.set_source_rgba(byte(r), byte(g), byte(b), byte(a));
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
    cr.set_line_width(1.0);
    cr.rectangle(0.5, 0.5, w - 1.0, h - 1.0);
    let _ = cr.stroke();
}

fn slider_row(icon: &str, tooltip: &str, scale: &gtk::Scale) -> gtk::Box {
    let image = gtk::Image::from_icon_name(icon);
    image.set_tooltip_text(Some(tooltip));
    image.add_css_class("dim-label");
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.append(&image);
    row.append(scale);
    row
}

/// Opaque colours are written without their alpha, so the common case reads
/// as the six digits people paste from anywhere else.
fn hex_of([r, g, b, a]: Rgba8) -> String {
    match a {
        255 => format!("#{r:02X}{g:02X}{b:02X}"),
        _ => format!("#{r:02X}{g:02X}{b:02X}{a:02X}"),
    }
}

fn parse_hex(text: &str) -> Option<Rgba8> {
    let digits = text.trim().trim_start_matches('#');
    if !digits.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    match digits.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

fn hsv_to_rgb(hue: f64, saturation: f64, value: f64) -> [u8; 3] {
    let sector = hue.rem_euclid(360.0) / 60.0;
    let chroma = value * saturation.clamp(0.0, 1.0);
    let rise = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, rise, 0.0),
        1 => (rise, chroma, 0.0),
        2 => (0.0, chroma, rise),
        3 => (0.0, rise, chroma),
        4 => (rise, 0.0, chroma),
        _ => (chroma, 0.0, rise),
    };
    let floor = value.clamp(0.0, 1.0) - chroma;
    let byte = |channel: f64| ((channel + floor) * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
}

fn rgb_to_hsv([r, g, b]: [u8; 3]) -> (f64, f64, f64) {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let value = r.max(g).max(b);
    let span = value - r.min(g).min(b);
    let hue = match value {
        _ if span == 0.0 => 0.0,
        _ if value == r => 60.0 * ((g - b) / span),
        _ if value == g => 60.0 * ((b - r) / span + 2.0),
        _ => 60.0 * ((r - g) / span + 4.0),
    };
    let saturation = if value == 0.0 { 0.0 } else { span / value };
    (hue.rem_euclid(360.0), saturation, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_survives_the_trip_through_hsv() {
        // The picker stores HSV and hands back RGB, so a colour the model set
        // has to come out of `rgba()` as the same bytes it went in as.
        for colour in QUICK {
            let [r, g, b, _] = colour;
            let (hue, saturation, value) = rgb_to_hsv([r, g, b]);
            assert_eq!(hsv_to_rgb(hue, saturation, value), [r, g, b], "{colour:?}");
        }
        for channel in 0..=255u8 {
            let grey = [channel, channel, channel];
            let (hue, saturation, value) = rgb_to_hsv(grey);
            assert_eq!(hsv_to_rgb(hue, saturation, value), grey);
        }
    }

    #[test]
    fn the_wheel_maps_a_hue_to_the_pixel_it_was_drawn_at() {
        // The cursor's placement is the inverse of the wheel's shading; if the
        // two disagree the marker sits on a colour that is not the one in hand.
        for hue in [0.0, 45.0, 120.0, 200.0, 359.0] {
            let angle = hue / 360.0 * TAU - PI;
            let (dx, dy) = (-RADIUS * 0.5 * angle.cos(), RADIUS * 0.5 * angle.sin());
            assert!((hue_at(dx, dy) - hue).abs() < 0.001, "{hue}");
        }
    }

    #[test]
    fn hex_round_trips_and_rejects_the_rest() {
        assert_eq!(parse_hex("#FF3C3C"), Some([255, 60, 60, 255]));
        assert_eq!(parse_hex("ff3c3c80"), Some([255, 60, 60, 128]));
        assert_eq!(hex_of([255, 60, 60, 255]), "#FF3C3C");
        assert_eq!(hex_of([255, 60, 60, 128]), "#FF3C3C80");
        for bad in ["", "#12345", "#GGGGGG", "#FF3C3C3", "rgb(1,2,3)"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
    }
}
