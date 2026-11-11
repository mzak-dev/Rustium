//! Material graphs: how a Scene's Colors are shaded.
//!
//! A Color (a [`Color`](crate::Color) palette entry) points at exactly one [`MaterialGraph`]; index 0
//! of the Scene's list is the built-in Default material. For now a graph is just constant Surface parameters;
//! the base color comes from the voxel's Color. The Effects milestone replaces the constants with a node graph
//! compiled to WGSL, behind the same `evaluate_material` entry point.

use serde::{Deserialize, Serialize};

/// The constant parameters of a graph's Surface output.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceConstants {
    /// Surface roughness, `0.0` (mirror) to `1.0` (matte).
    pub roughness: f32,
    /// `0.0` for dielectrics (paint, plastic) up to `1.0` for bare metal.
    pub metallic: f32,
    /// Color of the light the voxel emits, as 8-bit sRGB.
    pub emission_color: [u8; 3],
    /// Brightness of the emitted light; `0.0` means the voxel does not glow.
    pub emission_strength: f32,
    /// How much light passes through instead of bouncing off, `0.0` (opaque) to `1.0` (clear glass).
    pub transmission: f32,
    /// Index of refraction used when `transmission` is above zero.
    pub ior: f32,
}

impl SurfaceConstants {
    /// Largest emission strength.
    pub const MAX_EMISSION: f32 = 32.0;
    /// Smallest and largest index of refraction.
    pub const IOR_RANGE: (f32, f32) = (1.0, 3.0);

    /// An opaque, matte, non-emissive surface.
    pub const MATTE: Self = Self {
        roughness: 0.8,
        metallic: 0.0,
        emission_color: [255, 255, 255],
        emission_strength: 0.0,
        transmission: 0.0,
        ior: 1.5,
    };

    /// The same constants with every parameter pulled into its valid range (NaN falls back to the matte default).
    #[must_use]
    pub fn sanitized(self) -> Self {
        let clamp = |value: f32, min: f32, max: f32, fallback: f32| {
            if value.is_nan() {
                fallback
            } else {
                value.clamp(min, max)
            }
        };
        Self {
            roughness: clamp(self.roughness, 0.0, 1.0, Self::MATTE.roughness),
            metallic: clamp(self.metallic, 0.0, 1.0, Self::MATTE.metallic),
            emission_strength: clamp(self.emission_strength, 0.0, Self::MAX_EMISSION, 0.0),
            transmission: clamp(self.transmission, 0.0, 1.0, 0.0),
            ior: clamp(self.ior, Self::IOR_RANGE.0, Self::IOR_RANGE.1, Self::MATTE.ior),
            ..self
        }
    }
}

/// A named Material: what a Color looks like beyond its base color.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialGraph {
    /// Name shown in the interface, for example "Roof slate".
    pub name: String,
    /// The Surface output's constants.
    pub surface: SurfaceConstants,
}

impl MaterialGraph {
    /// Most graphs one Scene can hold (the GPU table has this many records).
    pub const MAX_PER_SCENE: usize = 64;

    /// The built-in Default material every Scene starts with at index 0.
    pub fn default_material() -> Self {
        Self {
            name: "Default material".to_owned(),
            surface: SurfaceConstants::MATTE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_clamps_every_parameter_and_replaces_nan() {
        let wild = SurfaceConstants {
            roughness: -3.0,
            metallic: 7.0,
            emission_strength: 1e9,
            transmission: f32::NAN,
            ior: 0.2,
            ..SurfaceConstants::MATTE
        };
        let clean = wild.sanitized();
        assert_eq!(clean.roughness.to_bits(), 0.0_f32.to_bits());
        assert_eq!(clean.metallic.to_bits(), 1.0_f32.to_bits());
        assert_eq!(
            clean.emission_strength.to_bits(),
            SurfaceConstants::MAX_EMISSION.to_bits()
        );
        assert_eq!(clean.transmission.to_bits(), 0.0_f32.to_bits());
        assert_eq!(clean.ior.to_bits(), 1.0_f32.to_bits());
    }

    #[test]
    fn the_default_material_is_matte() {
        assert_eq!(MaterialGraph::default_material().surface, SurfaceConstants::MATTE);
    }
}
