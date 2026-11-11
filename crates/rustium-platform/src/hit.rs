//! Window-region classification, independent of the operating system so it can be unit-tested anywhere.

/// An axis-aligned rectangle in physical pixels, relative to the window's client area.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Left edge.
    pub left: f32,
    /// Top edge.
    pub top: f32,
    /// Right edge.
    pub right: f32,
    /// Bottom edge.
    pub bottom: f32,
}

impl Rect {
    /// Whether the point lies inside the rectangle (right and bottom edges excluded).
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Where the frameless window's non-client regions are, as reported by the app every frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HitRegions {
    /// Client-area size in physical pixels.
    pub size: (f32, f32),
    /// Width of the resize band along the window border, in physical pixels.
    pub resize_border: f32,
    /// The top strip that drags the window.
    pub caption: Option<Rect>,
    /// Interactive controls inside the caption strip, which must keep receiving clicks.
    pub controls: Vec<Rect>,
    /// The maximize button; hovering it shows Snap Layouts on Windows 11.
    pub maximize_button: Option<Rect>,
}

/// A window edge or corner that resizes the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// Left edge.
    Left,
    /// Right edge.
    Right,
    /// Top edge.
    Top,
    /// Bottom edge.
    Bottom,
    /// Top-left corner.
    TopLeft,
    /// Top-right corner.
    TopRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-right corner.
    BottomRight,
}

/// What a window point belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitZone {
    /// Ordinary client area; the app handles input itself.
    Client,
    /// The drag strip.
    Caption,
    /// The maximize button.
    MaximizeButton,
    /// A resize edge or corner.
    Resize(Edge),
}

impl HitRegions {
    /// Classifies the client-relative point `(x, y)`. Resize edges are skipped while `maximized`.
    pub fn classify(&self, x: f32, y: f32, maximized: bool) -> HitZone {
        if !maximized && let Some(edge) = self.edge_at(x, y) {
            return HitZone::Resize(edge);
        }
        if self.maximize_button.is_some_and(|rect| rect.contains(x, y)) {
            return HitZone::MaximizeButton;
        }
        if self.controls.iter().any(|rect| rect.contains(x, y)) {
            return HitZone::Client;
        }
        if self.caption.is_some_and(|rect| rect.contains(x, y)) {
            return HitZone::Caption;
        }
        HitZone::Client
    }

    fn edge_at(&self, x: f32, y: f32) -> Option<Edge> {
        let (width, height) = self.size;
        if width <= 0.0 || height <= 0.0 || x < 0.0 || y < 0.0 || x >= width || y >= height {
            return None;
        }
        let band = self.resize_border;
        let west = x < band;
        let east = width - x <= band;
        let north = y < band;
        let south = height - y <= band;
        match (north, south, west, east) {
            (true, _, true, _) => Some(Edge::TopLeft),
            (true, _, _, true) => Some(Edge::TopRight),
            (_, true, true, _) => Some(Edge::BottomLeft),
            (_, true, _, true) => Some(Edge::BottomRight),
            (true, ..) => Some(Edge::Top),
            (_, true, ..) => Some(Edge::Bottom),
            (_, _, true, _) => Some(Edge::Left),
            (_, _, _, true) => Some(Edge::Right),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions() -> HitRegions {
        HitRegions {
            size: (1000.0, 700.0),
            resize_border: 6.0,
            caption: Some(Rect {
                left: 0.0,
                top: 0.0,
                right: 1000.0,
                bottom: 60.0,
            }),
            controls: vec![Rect {
                left: 800.0,
                top: 10.0,
                right: 990.0,
                bottom: 50.0,
            }],
            maximize_button: Some(Rect {
                left: 900.0,
                top: 14.0,
                right: 932.0,
                bottom: 46.0,
            }),
        }
    }

    #[test]
    fn empty_strip_drags_and_controls_stay_clickable() {
        let regions = regions();
        assert_eq!(regions.classify(400.0, 30.0, false), HitZone::Caption);
        assert_eq!(regions.classify(820.0, 30.0, false), HitZone::Client);
        assert_eq!(regions.classify(400.0, 300.0, false), HitZone::Client);
    }

    #[test]
    fn maximize_button_wins_over_the_controls_around_it() {
        assert_eq!(regions().classify(910.0, 30.0, false), HitZone::MaximizeButton);
    }

    #[test]
    fn border_resizes_unless_maximized() {
        let regions = regions();
        assert_eq!(regions.classify(2.0, 300.0, false), HitZone::Resize(Edge::Left));
        assert_eq!(regions.classify(2.0, 2.0, false), HitZone::Resize(Edge::TopLeft));
        assert_eq!(
            regions.classify(997.0, 698.0, false),
            HitZone::Resize(Edge::BottomRight)
        );
        assert_eq!(regions.classify(2.0, 300.0, true), HitZone::Client);
    }
}
