//! Line glyphs for the remote tiles (Roam style: thin, rounded strokes).
//!
//! Drawn with cairo in the widget's CSS `color`, so the stylesheet decides
//! purple on the plain tiles and white on the Power tile, per theme. Doing it
//! by hand (instead of icon-theme names) is what lets Volume Down / Up carry a
//! large, unmistakable `−` / `+` next to a small speaker.

use std::f64::consts::PI;

use adw::prelude::*;
use gtk::cairo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Back,
    Power,
    Home,
    Up,
    Down,
    Left,
    Right,
    Replay,
    Options,
    Search,
    Rewind,
    PlayPause,
    Forward,
    VolumeDown,
    Mute,
    VolumeUp,
}

impl Glyph {
    /// Logical drawing size in px (width, height).
    fn size(self) -> (i32, i32) {
        match self {
            Glyph::Power => (26, 26),
            Glyph::VolumeDown | Glyph::VolumeUp => (30, 22),
            _ => (22, 22),
        }
    }
}

/// A non-interactive drawing area showing `glyph` in the current CSS color.
pub fn widget(glyph: Glyph) -> gtk::DrawingArea {
    let (w, h) = glyph.size();
    let area = gtk::DrawingArea::builder()
        .content_width(w)
        .content_height(h)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .can_target(false)
        .build();
    area.add_css_class("glyph");
    area.set_draw_func(move |area, cr, w, h| {
        let c = area.color();
        cr.set_source_rgba(
            f64::from(c.red()),
            f64::from(c.green()),
            f64::from(c.blue()),
            f64::from(c.alpha()),
        );
        if let Err(e) = draw(cr, glyph, f64::from(w), f64::from(h)) {
            log::warn!("glyph {glyph:?}: {e}");
        }
    });
    // repaint when the colour can change (hover/press tint, theme switch)
    area.connect_state_flags_changed(|a, _| a.queue_draw());
    let weak = area.downgrade();
    adw::StyleManager::default().connect_dark_notify(move |_| {
        if let Some(a) = weak.upgrade() {
            a.queue_draw();
        }
    });
    area
}

fn poly(cr: &cairo::Context, pts: &[(f64, f64)], close: bool) {
    let Some((&(x0, y0), rest)) = pts.split_first() else {
        return;
    };
    cr.move_to(x0, y0);
    for &(x, y) in rest {
        cr.line_to(x, y);
    }
    if close {
        cr.close_path();
    }
}

/// Small speaker outline, 8 units wide, starting at `x`.
fn speaker(cr: &cairo::Context, x: f64) {
    poly(
        cr,
        &[
            (x, 9.5),
            (x + 3.2, 9.5),
            (x + 7.5, 5.8),
            (x + 7.5, 18.2),
            (x + 3.2, 14.5),
            (x, 14.5),
        ],
        true,
    );
}

/// Glyphs are designed on a 24-unit-high grid, horizontally centred.
fn draw(cr: &cairo::Context, glyph: Glyph, w: f64, h: f64) -> Result<(), cairo::Error> {
    let s = h / 24.0;
    let units_wide = w / s;
    cr.scale(s, s);
    cr.set_line_width(1.7);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    // every glyph below except the volume pair is drawn in a 24x24 box
    cr.translate((units_wide - 24.0) / 2.0, 0.0);
    match glyph {
        Glyph::Back => {
            poly(cr, &[(6.0, 12.0), (18.0, 12.0)], false);
            poly(cr, &[(11.0, 7.0), (6.0, 12.0), (11.0, 17.0)], false);
        }
        Glyph::Power => {
            cr.set_line_width(2.2);
            cr.new_sub_path();
            cr.arc(12.0, 12.8, 7.6, -50.0 * PI / 180.0, 230.0 * PI / 180.0);
            cr.stroke()?;
            poly(cr, &[(12.0, 3.6), (12.0, 11.2)], false);
        }
        Glyph::Home => {
            poly(cr, &[(3.8, 11.6), (12.0, 4.4), (20.2, 11.6)], false);
            poly(
                cr,
                &[(6.4, 9.6), (6.4, 19.6), (17.6, 19.6), (17.6, 9.6)],
                false,
            );
            cr.stroke()?;
            cr.rectangle(10.2, 14.2, 3.6, 5.4);
            cr.fill()?;
        }
        Glyph::Up => poly(cr, &[(7.0, 14.5), (12.0, 9.5), (17.0, 14.5)], false),
        Glyph::Down => poly(cr, &[(7.0, 9.5), (12.0, 14.5), (17.0, 9.5)], false),
        Glyph::Left => poly(cr, &[(14.5, 7.0), (9.5, 12.0), (14.5, 17.0)], false),
        Glyph::Right => poly(cr, &[(9.5, 7.0), (14.5, 12.0), (9.5, 17.0)], false),
        Glyph::Replay => {
            poly(cr, &[(10.5, 4.8), (7.0, 8.3), (10.5, 11.8)], false);
            cr.move_to(7.0, 8.3);
            cr.line_to(13.8, 8.3);
            cr.arc(13.8, 12.95, 4.65, -PI / 2.0, PI / 2.0);
            cr.line_to(8.5, 17.6);
        }
        Glyph::Options => {
            let (dx, dy) = (7.0 * (PI / 6.0).cos(), 3.5);
            poly(cr, &[(12.0, 5.0), (12.0, 19.0)], false);
            poly(cr, &[(12.0 - dx, 12.0 - dy), (12.0 + dx, 12.0 + dy)], false);
            poly(cr, &[(12.0 - dx, 12.0 + dy), (12.0 + dx, 12.0 - dy)], false);
        }
        Glyph::Search => {
            cr.new_sub_path();
            cr.arc(10.5, 10.5, 5.6, 0.0, 2.0 * PI);
            poly(cr, &[(14.7, 14.7), (19.3, 19.3)], false);
        }
        Glyph::Rewind => {
            poly(cr, &[(4.5, 12.0), (11.8, 7.2), (11.8, 16.8)], true);
            poly(cr, &[(11.8, 12.0), (19.1, 7.2), (19.1, 16.8)], true);
        }
        Glyph::Forward => {
            poly(cr, &[(19.5, 12.0), (12.2, 7.2), (12.2, 16.8)], true);
            poly(cr, &[(12.2, 12.0), (4.9, 7.2), (4.9, 16.8)], true);
        }
        Glyph::PlayPause => {
            poly(cr, &[(4.8, 7.2), (4.8, 16.8), (12.4, 12.0)], true);
            poly(cr, &[(15.8, 7.6), (15.8, 16.4)], false);
            poly(cr, &[(19.4, 7.6), (19.4, 16.4)], false);
        }
        Glyph::Mute => {
            speaker(cr, 7.2);
            cr.stroke()?;
            poly(cr, &[(5.0, 5.0), (19.0, 19.0)], false);
        }
        Glyph::VolumeDown | Glyph::VolumeUp => {
            // 30x22 px area: undo the 24-box centring, lay out on the full width
            cr.translate(-(units_wide - 24.0) / 2.0, 0.0);
            let left = (units_wide - 26.0) / 2.0;
            speaker(cr, left);
            cr.stroke()?;
            // the sign is bigger and bolder than the speaker on purpose
            cr.set_line_width(2.6);
            let (x0, x1) = (left + 13.0, left + 26.0);
            poly(cr, &[(x0, 12.0), (x1, 12.0)], false);
            if glyph == Glyph::VolumeUp {
                let mid = (x0 + x1) / 2.0;
                poly(cr, &[(mid, 5.5), (mid, 18.5)], false);
            }
        }
    }
    cr.stroke()
}
