use crate::settings::{Theme, Visualizer};
use gtk::prelude::*;
use gtk::DrawingArea;
use std::{cell::{Cell, RefCell}, rc::Rc};

pub const BANDS: usize = 32;
// Analyze many linear FFT bins, then draw 32 logarithmically-spaced columns.
// Keeping GStreamer's spectrum at 32 bands caused a near-permanent left-to-right
// decline because most music has more energy in its low-frequency FFT bins.
pub const FFT_BANDS: usize = 512;

pub struct Spectrum {
    pub widget: DrawingArea,
    values: Rc<RefCell<[f64; BANDS]>>,
    mode: Rc<Cell<Visualizer>>,
    theme: Rc<Cell<Theme>>,
}

fn colors(theme: Theme) -> ((f64, f64, f64), (f64, f64, f64)) {
    match theme {
        Theme::Graphite => ((0.52, 0.86, 0.70), (0.17, 0.25, 0.23)),
        Theme::Classic => ((0.46, 0.97, 0.35), (0.14, 0.25, 0.13)),
        Theme::Amber => ((1.00, 0.72, 0.33), (0.28, 0.22, 0.15)),
        Theme::Ice => ((0.07, 0.51, 0.69), (0.78, 0.87, 0.92)),
        Theme::Ocean => ((0.19, 0.84, 0.96), (0.10, 0.25, 0.34)),
        Theme::Violet => ((0.77, 0.46, 1.00), (0.25, 0.17, 0.34)),
        Theme::Silver => ((0.25, 0.42, 0.24), (0.70, 0.78, 0.68)),
        Theme::Phosphor => ((0.38, 1.00, 0.27), (0.05, 0.19, 0.05)),
        Theme::AdwaitaLight => ((0.21, 0.52, 0.89), (0.81, 0.85, 0.90)),
        Theme::AdwaitaDark => ((0.48, 0.69, 0.92), (0.24, 0.28, 0.34)),
        Theme::Nord => ((0.53, 0.75, 0.82), (0.27, 0.33, 0.40)),
        Theme::Catppuccin => ((0.71, 0.75, 0.98), (0.23, 0.23, 0.33)),
        Theme::SolarizedLight => ((0.15, 0.55, 0.82), (0.81, 0.81, 0.73)),
    }
}

impl Spectrum {
    pub fn new(mode: Visualizer, theme: Theme) -> Self {
        let values = Rc::new(RefCell::new([0.0; BANDS]));
        let mode_ref = Rc::new(Cell::new(mode));
        let theme_ref = Rc::new(Cell::new(theme));
        let widget = DrawingArea::new();
        let initial_height = if mode == Visualizer::Radial { 144 } else { 84 };
        widget.set_content_height(initial_height);
        widget.set_content_width(320);
        widget.set_height_request(initial_height);
        widget.set_vexpand(false);
        widget.set_hexpand(true);
        widget.set_visible(mode != Visualizer::Off);
        widget.add_css_class("spectrum");
        let display = values.clone();
        let selected_mode = mode_ref.clone();
        let selected_theme = theme_ref.clone();
        widget.set_draw_func(move |_, cr, width, height| {
            let w = width as f64;
            let h = height as f64;
            if w < 1.0 || h < 1.0 { return; }
            let levels = display.borrow();
            let (active, inactive) = colors(selected_theme.get());
            let pitch = w / BANDS as f64;
            let bar_width = (pitch * 0.60).clamp(2.0, 13.0);
            match selected_mode.get() {
                Visualizer::Segments => {
                    let segment_pitch = 5.0;
                    let count = ((h - 10.0) / segment_pitch).floor().max(1.0) as usize;
                    for (i, level) in levels.iter().enumerate() {
                        let x = i as f64 * pitch + (pitch - bar_width) * 0.5;
                        let lit = (level * count as f64).round() as usize;
                        for j in 0..count {
                            let y = h - 5.0 - (j as f64 + 1.0) * segment_pitch;
                            let rgb = if j >= lit { inactive }
                                else if j >= count * 9 / 10 { (0.96, 0.37, 0.32) }
                                else if j >= count * 3 / 4 { (0.95, 0.74, 0.34) }
                                else { active };
                            cr.set_source_rgb(rgb.0, rgb.1, rgb.2);
                            cr.rectangle(x, y, bar_width, 3.5);
                            let _ = cr.fill();
                        }
                    }
                }
                Visualizer::Bars => {
                    for (i, level) in levels.iter().enumerate() {
                        let x = i as f64 * pitch + (pitch - bar_width) * 0.5;
                        cr.set_source_rgb(active.0, active.1, active.2);
                        cr.rectangle(x, h - 4.0 - level * (h - 8.0), bar_width, level * (h - 8.0));
                        let _ = cr.fill();
                    }
                }
                // A frequency-response curve, not a simulated audio waveform.
                Visualizer::Curve => {
                    cr.set_source_rgb(active.0, active.1, active.2);
                    cr.set_line_width(2.0);
                    for (i, level) in levels.iter().enumerate() {
                        let x = (i as f64 + 0.5) * pitch;
                        let y = h - 5.0 - level * (h - 10.0);
                        if i == 0 { cr.move_to(x, y); } else { cr.line_to(x, y); }
                    }
                    let _ = cr.stroke();
                }
                // Frequency data reflected around the midline. Unlike a
                // waveform, this is still the FFT, with real audio levels.
                Visualizer::Mirror => {
                    let cy = h / 2.0;
                    let max_half = (h / 2.0 - 5.0).max(1.0);
                    cr.set_source_rgba(active.0, active.1, active.2, 0.20);
                    cr.set_line_width(1.0);
                    cr.move_to(0.0, cy);
                    cr.line_to(w, cy);
                    let _ = cr.stroke();
                    cr.set_source_rgb(active.0, active.1, active.2);
                    for (i, level) in levels.iter().enumerate() {
                        let x = i as f64 * pitch + (pitch - bar_width) * 0.5;
                        let half_height = (level * max_half).max(0.0);
                        cr.rectangle(x, cy - half_height, bar_width, half_height * 2.0);
                        let _ = cr.fill();
                    }
                }
                // Real frequency levels rendered as a circular 32-band ring.
                // More room is reserved for the drawing in radial mode.
                Visualizer::Radial => {
                    let cx = w / 2.0;
                    let cy = h / 2.0;
                    let radius = h.min(w) * 0.19;
                    let span = h.min(w) * 0.24;
                    cr.set_source_rgba(active.0, active.1, active.2, 0.32);
                    cr.set_line_width(1.5);
                    cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
                    let _ = cr.stroke();
                    cr.set_source_rgb(active.0, active.1, active.2);
                    cr.set_line_width(3.0);
                    cr.set_line_cap(gtk::cairo::LineCap::Round);
                    for (i, level) in levels.iter().enumerate() {
                        let theta = std::f64::consts::TAU * i as f64 / BANDS as f64
                            - std::f64::consts::FRAC_PI_2;
                        let length = 2.0 + level * span;
                        let inner = radius + 5.0;
                        let outer = inner + length;
                        let (dx, dy) = (theta.cos(), theta.sin());
                        cr.move_to(cx + dx * inner, cy + dy * inner);
                        cr.line_to(cx + dx * outer, cy + dy * outer);
                        let _ = cr.stroke();
                    }
                }
                Visualizer::Off => {}
            }
        });
        Self { widget, values, mode: mode_ref, theme: theme_ref }
    }

    pub fn set_mode(&self, mode: Visualizer) {
        self.mode.set(mode);
        let height = if mode == Visualizer::Radial { 144 } else { 84 };
        self.widget.set_content_height(height);
        self.widget.set_height_request(height);
        self.widget.set_visible(mode != Visualizer::Off);
        self.widget.queue_draw();
    }

    pub fn set_theme(&self, theme: Theme) {
        self.theme.set(theme);
        self.widget.queue_draw();
    }

    pub fn update(&self, db: &[f64]) {
        if db.is_empty() { return; }

        // GStreamer spectrum emits linearly spaced FFT bins. A full 17-20 kHz
        // display range puts the final column almost entirely in the region
        // where many compressed/mastered songs have little signal. Instead
        // keep 32 LOG-spaced columns over a useful musical range. The last
        // column now represents ~10-12 kHz, not near-ultrasonic noise.
        // This is only a visual range; GStreamer still plays full bandwidth.
        const MIN_HZ: f64 = 45.0;
        const MAX_HZ: f64 = 12_000.0;
        const NYQUIST_HZ: f64 = 24_000.0;
        let mut values = self.values.borrow_mut();
        for (i, value) in values.iter_mut().enumerate() {
            let lo = MIN_HZ * (MAX_HZ / MIN_HZ).powf(i as f64 / BANDS as f64);
            let hi = MIN_HZ * (MAX_HZ / MIN_HZ).powf((i + 1) as f64 / BANDS as f64);
            let from = ((lo / NYQUIST_HZ) * db.len() as f64).floor() as usize;
            let to = (((hi / NYQUIST_HZ) * db.len() as f64).ceil() as usize)
                .max(from + 1).min(db.len());
            let from = from.min(db.len() - 1);
            let bins = &db[from..to.max(from + 1)];
            let peak = bins.iter().copied().fold(-100.0_f64, f64::max);
            let avg = bins.iter().sum::<f64>() / bins.len() as f64;
            let level_db = peak * 0.65 + avg * 0.35;

            // Typical musical recordings have roughly pink-noise energy tilt:
            // without any compensation, high columns *always* look empty.
            // A gentle ~3 dB/octave display tilt counteracts that bias; it does
            // NOT change the audio or the equalizer. Do not amplify silence.
            let center = (lo * hi).sqrt();
            let tilt_db = 3.8 * (center / 90.0).log2().max(0.0);
            // -110 dB is the analyzer's actual silence floor. Do not light
            // a column with no measured energy, but preserve quiet cymbals,
            // harmonics and other highs previously clamped away at -90 dB.
            let target = if peak <= -109.5 { 0.0 } else {
                ((level_db + tilt_db + 88.0) / 72.0).clamp(0.0, 1.0).powf(0.95)
            };
            *value = if target > *value { *value * 0.25 + target * 0.75 }
                     else { *value * 0.74 + target * 0.26 };
        }
        self.widget.queue_draw();
    }

    pub fn clear(&self) {
        *self.values.borrow_mut() = [0.0; BANDS];
        self.widget.queue_draw();
    }
}
