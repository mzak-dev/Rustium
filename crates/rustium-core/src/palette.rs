#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "HSV to 8-bit RGB conversion rounds values already clamped to 0..=255"
)]

use std::num::NonZeroU8;

/// Index of a [`Material`] in the Palette; voxels store it in one byte (0 means empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MaterialId(NonZeroU8);

impl MaterialId {
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
///
/// ponytail: a later milestone renames this to `Color` and `MaterialId` to `ColorId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Material {
    /// Base color as 8-bit sRGB.
    pub color: [u8; 3],
    /// Index into the Scene's material graphs; 0 is the Default material.
    pub graph: u32,
}

impl Material {
    /// A Color of `color` shaded by the Default material.
    pub fn new(color: [u8; 3]) -> Self {
        Self { color, graph: 0 }
    }
}

/// The ordered list of up to [`Palette::MAX_LEN`] Materials a Scene can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    materials: Vec<Material>,
}

impl Palette {
    /// Most Materials one Palette can hold.
    pub const MAX_LEN: usize = 255;

    /// Changes the color of `id`; returns whether anything changed.
    pub(crate) fn set_color(&mut self, id: MaterialId, color: [u8; 3]) -> bool {
        match self.materials.get_mut(id.slot()) {
            Some(material) if material.color != color => {
                material.color = color;
                true
            }
            _ => false,
        }
    }

    /// Material for `id`, if the Palette has that many entries.
    pub fn get(&self, id: MaterialId) -> Option<&Material> {
        self.materials.get(id.slot())
    }

    /// Mutable access to the Material for `id`; only Commands change the Palette.
    pub(crate) fn get_mut(&mut self, id: MaterialId) -> Option<&mut Material> {
        self.materials.get_mut(id.slot())
    }

    /// Number of Materials.
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    /// Whether the Palette has no Materials.
    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// Every Material with its id, in Palette order.
    pub fn iter(&self) -> impl Iterator<Item = (MaterialId, &Material)> {
        (1..=u8::MAX).filter_map(MaterialId::new).zip(&self.materials)
    }

    /// The Material whose color is closest to `color`.
    pub fn nearest(&self, color: [u8; 3]) -> Option<MaterialId> {
        let distance = |material: &Material| -> i32 {
            material
                .color
                .iter()
                .zip(color)
                .map(|(a, b)| (i32::from(*a) - i32::from(b)).pow(2))
                .sum()
        };
        self.iter()
            .min_by_key(|(_, material)| distance(material))
            .map(|(id, _)| id)
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
        let mut materials = Vec::with_capacity(96);
        for value in GREYS {
            materials.push(Material::new(hsv_to_rgb(0.0, 0.0, value)));
        }
        for hue in HUES {
            for (saturation, value) in SHADES {
                materials.push(Material::new(hsv_to_rgb(hue, saturation, value)));
            }
        }
        Self { materials }
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
        let first = MaterialId::new(1)
            .and_then(|id| palette.get(id))
            .map(|material| material.color);
        assert_eq!(first, Some([245, 245, 245]));
    }

    #[test]
    fn material_zero_does_not_exist() {
        assert_eq!(MaterialId::new(0), None);
    }
}
