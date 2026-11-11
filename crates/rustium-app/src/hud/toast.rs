//! A short message under the top bar, e.g. the result of a script run. Click it to dismiss.

use std::time::Duration;

use egui::{Align2, Color32, Context, RichText, vec2};

use super::{Hud, Intro, MARGIN, Place, TOP_BAR_HEIGHT, island};
use crate::theme;

/// Seconds a toast stays on screen.
const LIFETIME: f64 = 8.0;

/// Text shown in the toast; an error is drawn in a warm color.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Toast {
    text: String,
    error: bool,
    shown_at: Option<f64>,
}

impl Toast {
    pub(crate) fn new(text: String, error: bool) -> Self {
        Self {
            text,
            error,
            shown_at: None,
        }
    }
}

pub(crate) fn show(ctx: &Context, hud: &mut Hud<'_>) {
    let now = ctx.input(|input| input.time);
    let Some(toast) = &mut hud.ui.toast else {
        return;
    };
    let shown_at = *toast.shown_at.get_or_insert(now);
    if now - shown_at > LIFETIME {
        hud.ui.toast = None;
        return;
    }
    ctx.request_repaint_after(Duration::from_millis(500));
    let (text, error) = (toast.text.clone(), toast.error);
    let color = if error {
        Color32::from_rgb(0xf0, 0x8a, 0x72)
    } else {
        theme::TEXT
    };
    let response = island(
        ctx,
        hud,
        "toast",
        Place::Anchor(Align2::CENTER_TOP, vec2(0.0, MARGIN + TOP_BAR_HEIGHT + 12.0)),
        Intro {
            delay: 0.0,
            from: vec2(0.0, -8.0),
        },
        theme::island(),
        |ui, _| {
            ui.set_max_width(520.0);
            ui.label(RichText::new(text).color(color).size(12.0));
        },
    );
    if response.response.clicked() {
        hud.ui.toast = None;
    }
}
