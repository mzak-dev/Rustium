#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "HSV to 8-bit RGB conversion rounds values already clamped to 0..=255"
)]

use std::num::NonZeroU8;

/// Index of a [`Color`] in the Palette; voxels store it in one byte (0 means empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColorId(NonZeroU8);

impl ColorId {
    /// The first Palette entry, which every Palette has.
    pub const FIRST: Self = Self(NonZeroU8::MIN);

    /// The id with palette number `number` (1..=255); `None` for 0.
    pub fn new(number: u8) -> Option<Self> {
        NonZeroU8::new(number).map(Self)
    }

    /// Palette number, 1..=255.
    pub fn get(self) -> u8 {
        self.0.get()
    }

    fn slot(self) -> usize {
        usize::from(self.0.get()) - 1
    }
}

/// A Palette entry: a base color plus the index of the [`MaterialGraph`](crate::MaterialGraph) shading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// Base color as 8-bit sRGB.
    pub color: [u8; 3],
    /// Index into the Scene's material graphs; 0 is the Default material.
    pub graph: u32,
}

impl Color {
    /// A Color of `color` shaded by the Default material.
    pub fn new(color: [u8; 3]) -> Self {
        Self { color, graph: 0 }
    }
}

/// The ordered list of up to [`Palette::MAX_LEN`] Colors a Scene can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    colors: Vec<Color>,
}

impl Palette {
    /// Most Colors one Palette can hold.
    pub const MAX_LEN: usize = 255;

    /// A Palette of the given entries; `None` when there are none or more than [`Palette::MAX_LEN`].
    pub(crate) fn from_colors(colors: Vec<Color>) -> Option<Self> {
        (!colors.is_empty() && colors.len() <= Self::MAX_LEN).then_some(Self { colors })
    }

    /// Removes the Color `id`; later entries move down by one. Refuses to remove the last Color.
    pub(crate) fn remove(&mut self, id: ColorId) -> bool {
        if self.colors.len() <= 1 || id.slot() >= self.colors.len() {
            return false;
        }
        self.colors.remove(id.slot());
        true
    }

    /// Swaps the Colors `a` and `b`; returns false when either is missing or they are the same.
    pub(crate) fn swap(&mut self, a: ColorId, b: ColorId) -> bool {
        if a == b || a.slot().max(b.slot()) >= self.colors.len() {
            return false;
        }
        self.colors.swap(a.slot(), b.slot());
        true
    }

    /// Appends a Color with the Default material; returns false when the Palette is full.
    pub(crate) fn push(&mut self, color: [u8; 3]) -> bool {
        if self.colors.len() >= Self::MAX_LEN {
            return false;
        }
        self.colors.push(Color::new(color));
        true
    }

    /// Changes the color of `id`; returns whether anything changed.
    pub(crate) fn set_color(&mut self, id: ColorId, color: [u8; 3]) -> bool {
        match self.colors.get_mut(id.slot()) {
            Some(entry) if entry.color != color => {
                entry.color = color;
                true
            }
            _ => false,
        }
    }

    /// Color for `id`, if the Palette has that many entries.
    pub fn get(&self, id: ColorId) -> Option<&Color> {
        self.colors.get(id.slot())
    }

    /// Mutable access to the Color for `id`; only Commands change the Palette.
    pub(crate) fn get_mut(&mut self, id: ColorId) -> Option<&mut Color> {
        self.colors.get_mut(id.slot())
    }

    /// Number of Colors.
    pub fn len(&self) -> usize {
        self.colors.len()
    }

    /// Whether the Palette has no Colors.
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// Every Color with its id, in Palette order.
    pub fn iter(&self) -> impl Iterator<Item = (ColorId, &Color)> {
        (1..=u8::MAX).filter_map(ColorId::new).zip(&self.colors)
    }

    /// The Color whose base color is closest to `color`.
    pub fn nearest(&self, color: [u8; 3]) -> Option<ColorId> {
        let distance = |entry: &Color| -> i32 {
            entry
                .color
                .iter()
                .zip(color)
                .map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2))
                .sum()
        };
        self.iter().min_by_key(|(_, entry)| distance(entry)).map(|(id, _)| id)
    }
}

impl Default for Palette {
    /// The 96-color default palette: 8 greys followed by 11 hues in 8 shades each.
    fn default() -> Self {
        const GREYS: [f32; 8] = [0.96, 0.84, 0.72, 0.58, 0.44, 0.32, 0.2, 0.1];
        const HUES: [f32; 11] = [0.0, 22.0, 40.0, 52.0, 95.0, 140.0, 170.0, 195.0, 218.0, 262.0, 325.0];
        const SHADES: [(f32, f32); 8] = [
            (0.22, 1.0),
            (0.42, 0.97),
            (0.62, 0.92),
            (0.78, 0.82),
            (0.84, 0.68),
            (0.86, 0.53),
            (0.82, 0.38),
            (0.78, 0.24),
        ];
        let mut colors = Vec::with_capacity(96);
        for value in GREYS {
            colors.push(Color::new(hsv_to_rgb(0.0, 0.0, value)));
        }
        for hue in HUES {
            for (saturation, value) in SHADES {
                colors.push(Color::new(hsv_to_rgb(hue, saturation, value)));
            }
        }
        Self { colors }
    }
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> [u8; 3] {
    let channel = |n: f32| {
        let k = (n + hue / 60.0) % 6.0;
        let c = value - value * saturation * k.min(4.0 - k).clamp(0.0, 1.0);
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [channel(5.0), channel(3.0), channel(1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_has_96_colors_starting_with_light_grey() {
        let palette = Palette::default();
        assert_eq!(palette.len(), 96);
        let first = ColorId::new(1).and_then(|id| palette.get(id)).map(|color| color.color);
        assert_eq!(first, Some([245, 245, 245]));
    }

    #[test]
    fn color_zero_does_not_exist() {
        assert_eq!(ColorId::new(0), None);
    }
}
