//! The node library: every kind of node a Material graph can hold, with its sockets and parameters.
//!
//! The tables here are the single source of truth for the editor (what to draw, which widgets to show), for graph
//! validation and for the WGSL compiler in `rustium-render`.

use serde::{Deserialize, Serialize};

/// The data type of a socket. Any type converts to any other, like in Blender.
///
/// - Float to Color or Vector: the value in all three channels (grey).
/// - Color to Float: luminance. Vector to Float: the average of the three components.
/// - Color and Vector convert channel by channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SocketType {
    /// Linear RGB color.
    Color,
    /// Three components, for example a position or a normal.
    Vector,
    /// A single number.
    Float,
}

impl SocketType {
    /// Colour of the wire and the socket dot in the editor, as 8-bit sRGB.
    pub const fn wire_color(self) -> [u8; 3] {
        match self {
            Self::Color => [0xe6, 0xc3, 0x5c],
            Self::Vector => [0x8f, 0xa8, 0xf7],
            Self::Float => [0xb9, 0xc0, 0xcb],
        }
    }
}

/// The group a node belongs to in the library and in the node header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeCategory {
    /// Where the voxel's own data enters the graph.
    Input,
    /// Procedural patterns.
    Pattern,
    /// Color operations.
    Color,
    /// Math on numbers.
    Math,
    /// Shading-only helpers such as the voxel bevel (the mock lists these under "Surface").
    Shading,
    /// The nodes that end a graph.
    Output,
}

impl NodeCategory {
    /// Every category, in library order.
    pub const ALL: [Self; 6] = [
        Self::Input,
        Self::Pattern,
        Self::Color,
        Self::Math,
        Self::Shading,
        Self::Output,
    ];

    /// English name of the category.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Pattern => "Pattern",
            Self::Color => "Color",
            Self::Math => "Math",
            Self::Shading => "Surface",
            Self::Output => "Output",
        }
    }

    /// Colour of the dot in the node header, as 8-bit sRGB.
    pub const fn dot_color(self) -> [u8; 3] {
        match self {
            Self::Input => [0x6f, 0xb3, 0x6a],
            Self::Pattern => [0xb7, 0x9c, 0xf2],
            Self::Color => [0xe6, 0xc3, 0x5c],
            Self::Math => [0x7f, 0xb2, 0xd9],
            Self::Shading => [0xe0, 0x8a, 0x6a],
            Self::Output => [0xee, 0x6c, 0x3e],
        }
    }
}

/// What an input socket holds while nothing is linked to it. Colors are stored as sRGB `0.0..=1.0`; the compiler
/// converts them to linear.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InputValue {
    /// A constant number (Float sockets).
    Float(f32),
    /// A constant vector (Vector sockets).
    Vector([f32; 3]),
    /// A constant sRGB color (Color sockets).
    Color([f32; 3]),
    /// The position of the shaded point in the Scene, in voxels (Vector sockets).
    Position,
    /// The geometric normal of the shaded point (Vector sockets).
    Normal,
    /// The base color of the voxel's Palette Color (Color sockets).
    VoxelColor,
}

impl InputValue {
    /// Whether this value can sit on a socket of type `ty`.
    pub const fn fits(self, ty: SocketType) -> bool {
        matches!(
            (self, ty),
            (Self::Float(_), SocketType::Float)
                | (Self::Vector(_) | Self::Position | Self::Normal, SocketType::Vector)
                | (Self::Color(_) | Self::VoxelColor, SocketType::Color)
        )
    }

    /// The same value with every number made finite and clamped to something sane.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let clean = |value: f32| {
            if value.is_finite() {
                value.clamp(-1.0e6, 1.0e6)
            } else {
                0.0
            }
        };
        let color = |value: f32| if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        match self {
            Self::Float(value) => Self::Float(clean(value)),
            Self::Vector(value) => Self::Vector(value.map(clean)),
            Self::Color(value) => Self::Color(value.map(color)),
            other => other,
        }
    }
}

/// One stop of a Color ramp.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RampStop {
    /// Where the stop sits, `0.0..=1.0`.
    pub position: f32,
    /// sRGB color at the stop.
    pub color: [f32; 3],
}

/// The value of a node parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    /// A number.
    Float(f32),
    /// A whole number.
    Int(i32),
    /// A switch.
    Bool(bool),
    /// The index of one of a fixed list of options.
    Choice(u32),
    /// A pair `[low, high]`.
    Range([f32; 2]),
    /// An sRGB color.
    Color([f32; 3]),
    /// The stops of a Color ramp, sorted by position.
    Stops(Vec<RampStop>),
}

/// Fewest and most stops a Color ramp may have.
pub const RAMP_STOPS: std::ops::RangeInclusive<usize> = 2..=8;

/// What a parameter is and which values it accepts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    /// A number in `min..=max`.
    Float {
        /// Smallest value.
        min: f32,
        /// Largest value.
        max: f32,
        /// Value of a new node.
        default: f32,
    },
    /// A whole number in `min..=max`.
    Int {
        /// Smallest value.
        min: i32,
        /// Largest value.
        max: i32,
        /// Value of a new node.
        default: i32,
    },
    /// A switch.
    Bool {
        /// Value of a new node.
        default: bool,
    },
    /// One of `options`.
    Choice {
        /// English names of the options.
        options: &'static [&'static str],
        /// Index chosen on a new node.
        default: u32,
    },
    /// A `[low, high]` pair, both in `min..=max`.
    Range {
        /// Smallest value either end may take.
        min: f32,
        /// Largest value either end may take.
        max: f32,
        /// Value of a new node.
        default: [f32; 2],
    },
    /// An sRGB color.
    Color {
        /// Value of a new node.
        default: [f32; 3],
    },
    /// Ramp stops: [`RAMP_STOPS`] of them.
    Stops,
}

impl ParamKind {
    /// The value a new node starts with.
    pub fn default_value(&self) -> ParamValue {
        match *self {
            Self::Float { default, .. } => ParamValue::Float(default),
            Self::Int { default, .. } => ParamValue::Int(default),
            Self::Bool { default } => ParamValue::Bool(default),
            Self::Choice { default, .. } => ParamValue::Choice(default),
            Self::Range { default, .. } => ParamValue::Range(default),
            Self::Color { default } => ParamValue::Color(default),
            Self::Stops => ParamValue::Stops(vec![
                RampStop {
                    position: 0.0,
                    color: [0.0, 0.0, 0.0],
                },
                RampStop {
                    position: 1.0,
                    color: [1.0, 1.0, 1.0],
                },
            ]),
        }
    }

    /// Checks that `value` is the right variant and pulls it into range (non-finite numbers fall back to the
    /// default). Returns `None` when the variant does not match.
    pub fn sanitize(&self, value: &ParamValue) -> Option<ParamValue> {
        let finite = |value: f32, min: f32, max: f32, fallback: f32| {
            if value.is_finite() {
                value.clamp(min, max)
            } else {
                fallback
            }
        };
        let color = |c: f32| if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 };
        Some(match (self, value) {
            (&Self::Float { min, max, default }, &ParamValue::Float(v)) => {
                ParamValue::Float(finite(v, min, max, default))
            }
            (&Self::Int { min, max, .. }, &ParamValue::Int(v)) => ParamValue::Int(v.clamp(min, max)),
            (Self::Bool { .. }, &ParamValue::Bool(v)) => ParamValue::Bool(v),
            (&Self::Choice { options, .. }, &ParamValue::Choice(v)) => {
                let last = u32::try_from(options.len().saturating_sub(1)).unwrap_or(0);
                ParamValue::Choice(v.min(last))
            }
            (&Self::Range { min, max, default }, &ParamValue::Range([a, b])) => {
                ParamValue::Range([finite(a, min, max, default[0]), finite(b, min, max, default[1])])
            }
            (Self::Color { .. }, &ParamValue::Color(v)) => ParamValue::Color(v.map(color)),
            (Self::Stops, ParamValue::Stops(stops)) => {
                if !RAMP_STOPS.contains(&stops.len()) {
                    return None;
                }
                let mut stops: Vec<RampStop> = stops
                    .iter()
                    .map(|stop| RampStop {
                        position: finite(stop.position, 0.0, 1.0, 0.0),
                        color: stop.color.map(color),
                    })
                    .collect();
                stops.sort_by(|a, b| a.position.total_cmp(&b.position));
                ParamValue::Stops(stops)
            }
            _ => return None,
        })
    }
}

/// An input socket of a node kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputSpec {
    /// Name shown next to the socket.
    pub name: &'static str,
    /// Data type.
    pub ty: SocketType,
    /// Value used while nothing is linked.
    pub default: InputValue,
    /// Smallest and largest value a constant may take (Float sockets; ignored otherwise).
    pub range: (f32, f32),
}

/// An output socket of a node kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputSpec {
    /// Name shown next to the socket.
    pub name: &'static str,
    /// Data type.
    pub ty: SocketType,
}

/// A parameter of a node kind: edited in the inspector, never linked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// Name shown next to the widget.
    pub name: &'static str,
    /// Type and limits.
    pub kind: ParamKind,
}

/// Everything the editor and the compiler need to know about one node kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeSpec {
    /// The kind this describes.
    pub kind: GraphNodeKind,
    /// Name shown in the library and as the default node title.
    pub label: &'static str,
    /// One-line description for the inspector.
    pub description: &'static str,
    /// Library group.
    pub category: NodeCategory,
    /// Input sockets, top to bottom.
    pub inputs: &'static [InputSpec],
    /// Output sockets, top to bottom.
    pub outputs: &'static [OutputSpec],
    /// Parameters, in inspector order.
    pub params: &'static [ParamSpec],
    /// With Bypass on, the node passes this input (converted) to its first output; `None` outputs zero (or, for the
    /// Voxel bevel, the geometric normal).
    pub bypass_input: Option<usize>,
}

/// Every kind of node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GraphNodeKind {
    /// The voxel's Color, position and normal.
    VoxelColor,
    /// Occlusion of the surface by its neighbours.
    AmbientOcclusion,
    /// A constant number.
    Value,
    /// Smooth random values.
    Noise,
    /// Cellular (Worley) pattern.
    Cells,
    /// A ramp along one axis.
    Gradient,
    /// A mask that is strong near voxel edges.
    EdgeWear,
    /// Blends two colors.
    Mix,
    /// Maps a number to a color through stops.
    ColorRamp,
    /// Rotates the hue of a color.
    HueShift,
    /// Maps a number from one range to another.
    Remap,
    /// Multiplies two numbers.
    Multiply,
    /// Limits a number to a range.
    Clamp,
    /// Rounds voxel edges in the shading only.
    VoxelBevel,
    /// The end of the graph: the surface.
    SurfaceOutput,
    /// The end of the graph: haze inside the voxel.
    VolumeOutput,
}

const NO_RANGE: (f32, f32) = (0.0, 0.0);
const UNIT: (f32, f32) = (0.0, 1.0);
const WIDE: (f32, f32) = (-1000.0, 1000.0);

const fn float_in(name: &'static str, default: f32, range: (f32, f32)) -> InputSpec {
    InputSpec {
        name,
        ty: SocketType::Float,
        default: InputValue::Float(default),
        range,
    }
}

const fn color_in(name: &'static str, default: InputValue) -> InputSpec {
    InputSpec {
        name,
        ty: SocketType::Color,
        default,
        range: NO_RANGE,
    }
}

const fn vector_in(name: &'static str, default: InputValue) -> InputSpec {
    InputSpec {
        name,
        ty: SocketType::Vector,
        default,
        range: NO_RANGE,
    }
}

const fn out(name: &'static str, ty: SocketType) -> OutputSpec {
    OutputSpec { name, ty }
}

const fn param(name: &'static str, kind: ParamKind) -> ParamSpec {
    ParamSpec { name, kind }
}

const fn float_param(name: &'static str, min: f32, max: f32, default: f32) -> ParamSpec {
    param(name, ParamKind::Float { min, max, default })
}

const SPACE: ParamKind = ParamKind::Choice {
    options: &["Model", "World"],
    default: 0,
};

const SEED: ParamKind = ParamKind::Int {
    min: 0,
    max: 9999,
    default: 0,
};

static VOXEL_COLOR: NodeSpec = NodeSpec {
    kind: GraphNodeKind::VoxelColor,
    label: "Voxel color",
    description: "The Palette Color of the voxel, and where on the model the shaded point is.",
    category: NodeCategory::Input,
    inputs: &[],
    outputs: &[
        out("Color", SocketType::Color),
        out("Position", SocketType::Vector),
        out("Normal", SocketType::Vector),
    ],
    params: &[],
    bypass_input: None,
};

static AMBIENT_OCCLUSION: NodeSpec = NodeSpec {
    kind: GraphNodeKind::AmbientOcclusion,
    label: "Ambient occlusion",
    description: "How open the surface is: 1 in the open, lower in corners. The path tracer reports 1.",
    category: NodeCategory::Input,
    inputs: &[],
    outputs: &[out("AO", SocketType::Float)],
    params: &[float_param("Strength", 0.0, 1.0, 1.0)],
    bypass_input: None,
};

static VALUE: NodeSpec = NodeSpec {
    kind: GraphNodeKind::Value,
    label: "Value",
    description: "A constant number.",
    category: NodeCategory::Input,
    inputs: &[],
    outputs: &[out("Value", SocketType::Float)],
    params: &[float_param("Value", -1000.0, 1000.0, 0.5)],
    bypass_input: None,
};

static NOISE: NodeSpec = NodeSpec {
    kind: GraphNodeKind::Noise,
    label: "Noise",
    description: "Smooth random values from a position. Scale 1 is one noise cell per 8 voxels.",
    category: NodeCategory::Pattern,
    inputs: &[vector_in("Vector", InputValue::Position)],
    outputs: &[out("Value", SocketType::Float)],
    params: &[
        float_param("Scale", 0.01, 64.0, 4.0),
        param(
            "Detail",
            ParamKind::Int {
                min: 0,
                max: 6,
                default: 2,
            },
        ),
        param("Seed", SEED),
        param("Space", SPACE),
        param("Per voxel", ParamKind::Bool { default: false }),
    ],
    bypass_input: None,
};

static CELLS: NodeSpec = NodeSpec {
    kind: GraphNodeKind::Cells,
    label: "Cells",
    description: "Cellular pattern: distance to the nearest point, a random value per cell and the distance to the \
                  cell border.",
    category: NodeCategory::Pattern,
    inputs: &[vector_in("Vector", InputValue::Position)],
    outputs: &[
        out("Distance", SocketType::Float),
        out("Cell", SocketType::Float),
        out("Border", SocketType::Float),
    ],
    params: &[
        float_param("Scale", 0.01, 64.0, 4.0),
        float_param("Randomness", 0.0, 1.0, 1.0),
        param("Seed", SEED),
        param("Space", SPACE),
    ],
    bypass_input: None,
};

static GRADIENT: NodeSpec = NodeSpec {
    kind: GraphNodeKind::Gradient,
    label: "Gradient",
    description: "Goes from 0 to 1 along an axis, between two positions given in voxels.",
    category: NodeCategory::Pattern,
    inputs: &[vector_in("Vector", InputValue::Position)],
    outputs: &[out("Value", SocketType::Float)],
    params: &[
        param(
            "Axis",
            ParamKind::Choice {
                options: &["X", "Y", "Z"],
                default: 1,
            },
        ),
        param(
            "Range",
            ParamKind::Range {
                min: -1000.0,
                max: 1000.0,
                default: [0.0, 16.0],
            },
        ),
        param("Space", SPACE),
    ],
    bypass_input: None,
};

static EDGE_WEAR: NodeSpec = NodeSpec {
    kind: GraphNodeKind::EdgeWear,
    label: "Edge wear",
    description: "A mask that is 1 on voxel edges and fades inwards; breakup makes it irregular.",
    category: NodeCategory::Pattern,
    inputs: &[],
    outputs: &[out("Mask", SocketType::Float)],
    params: &[
        float_param("Width", 0.01, 0.5, 0.12),
        float_param("Breakup", 0.0, 1.0, 0.4),
        param("Seed", SEED),
    ],
    bypass_input: None,
};

static MIX: NodeSpec = NodeSpec {
    kind: GraphNodeKind::Mix,
    label: "Mix",
    description: "Blends color B over color A by Factor.",
    category: NodeCategory::Color,
    inputs: &[
        float_in("Factor", 0.5, UNIT),
        color_in("A", InputValue::Color([0.5, 0.5, 0.5])),
        color_in("B", InputValue::Color([0.243, 0.290, 0.337])),
    ],
    outputs: &[out("Color", SocketType::Color)],
    params: &[param(
        "Blend",
        ParamKind::Choice {
            options: &["Mix", "Multiply", "Add", "Screen"],
            default: 0,
        },
    )],
    bypass_input: Some(1),
};

static COLOR_RAMP: NodeSpec = NodeSpec {
    kind: GraphNodeKind::ColorRamp,
    label: "Color ramp",
    description: "Turns a number into a color by interpolating between stops.",
    category: NodeCategory::Color,
    inputs: &[float_in("Factor", 0.5, UNIT)],
    outputs: &[out("Color", SocketType::Color)],
    params: &[param("Stops", ParamKind::Stops)],
    bypass_input: Some(0),
};

static HUE_SHIFT: NodeSpec = NodeSpec {
    kind: GraphNodeKind::HueShift,
    label: "Hue shift",
    description: "Rotates the hue of a color by Shift degrees and scales its saturation and value.",
    category: NodeCategory::Color,
    inputs: &[
        color_in("Color", InputValue::VoxelColor),
        float_in("Shift", 0.0, (-360.0, 360.0)),
    ],
    outputs: &[out("Color", SocketType::Color)],
    params: &[
        float_param("Saturation", 0.0, 2.0, 1.0),
        float_param("Value", 0.0, 2.0, 1.0),
    ],
    bypass_input: Some(0),
};

static REMAP: NodeSpec = NodeSpec {
    kind: GraphNodeKind::Remap,
    label: "Remap",
    description: "Maps a number from the From range to the To range.",
    category: NodeCategory::Math,
    inputs: &[float_in("Value", 0.0, WIDE)],
    outputs: &[out("Result", SocketType::Float)],
    params: &[
        param(
            "From",
            ParamKind::Range {
                min: -1000.0,
                max: 1000.0,
                default: [0.0, 1.0],
            },
        ),
        param(
            "To",
            ParamKind::Range {
                min: -1000.0,
                max: 1000.0,
                default: [0.0, 1.0],
            },
        ),
        param("Clamp", ParamKind::Bool { default: true }),
    ],
    bypass_input: Some(0),
};

static MULTIPLY: NodeSpec = NodeSpec {
    kind: GraphNodeKind::Multiply,
    label: "Multiply",
    description: "Multiplies two numbers.",
    category: NodeCategory::Math,
    inputs: &[float_in("A", 1.0, WIDE), float_in("B", 1.0, WIDE)],
    outputs: &[out("Result", SocketType::Float)],
    params: &[],
    bypass_input: Some(0),
};

static CLAMP: NodeSpec = NodeSpec {
    kind: GraphNodeKind::Clamp,
    label: "Clamp",
    description: "Limits a number to the range Min..Max.",
    category: NodeCategory::Math,
    inputs: &[
        float_in("Value", 0.5, WIDE),
        float_in("Min", 0.0, WIDE),
        float_in("Max", 1.0, WIDE),
    ],
    outputs: &[out("Result", SocketType::Float)],
    params: &[],
    bypass_input: Some(0),
};

static VOXEL_BEVEL: NodeSpec = NodeSpec {
    kind: GraphNodeKind::VoxelBevel,
    label: "Voxel bevel",
    description: "Tilts the normal near voxel edges so they catch light as if rounded. Shading only, the geometry \
                  stays.",
    category: NodeCategory::Shading,
    inputs: &[],
    outputs: &[out("Normal", SocketType::Vector)],
    params: &[
        float_param("Radius", 0.0, 0.5, 0.08),
        param(
            "Segments",
            ParamKind::Int {
                min: 1,
                max: 8,
                default: 2,
            },
        ),
    ],
    bypass_input: None,
};

static SURFACE_OUTPUT: NodeSpec = NodeSpec {
    kind: GraphNodeKind::SurfaceOutput,
    label: "Surface",
    description: "Ends the Material: how the voxel's surface looks.",
    category: NodeCategory::Output,
    inputs: &[
        color_in("Base color", InputValue::VoxelColor),
        float_in("Metallic", 0.0, UNIT),
        float_in("Roughness", 0.8, UNIT),
        vector_in("Normal", InputValue::Normal),
        color_in("Emission", InputValue::Color([1.0, 1.0, 1.0])),
        float_in("Emission strength", 0.0, (0.0, 32.0)),
        float_in("Transmission", 0.0, UNIT),
        float_in("IOR", 1.5, (1.0, 3.0)),
    ],
    outputs: &[],
    params: &[],
    bypass_input: None,
};

static VOLUME_OUTPUT: NodeSpec = NodeSpec {
    kind: GraphNodeKind::VolumeOutput,
    label: "Volume",
    description: "Optional end of the Material: haze inside the voxel.",
    category: NodeCategory::Output,
    inputs: &[
        float_in("Density", 0.0, (0.0, 64.0)),
        color_in("Scattering color", InputValue::Color([1.0, 1.0, 1.0])),
    ],
    outputs: &[],
    params: &[],
    bypass_input: None,
};

impl GraphNodeKind {
    /// Every kind, in library order.
    pub const ALL: [Self; 16] = [
        Self::VoxelColor,
        Self::AmbientOcclusion,
        Self::Value,
        Self::Noise,
        Self::Cells,
        Self::Gradient,
        Self::EdgeWear,
        Self::Mix,
        Self::ColorRamp,
        Self::HueShift,
        Self::Remap,
        Self::Multiply,
        Self::Clamp,
        Self::VoxelBevel,
        Self::SurfaceOutput,
        Self::VolumeOutput,
    ];

    /// Sockets, parameters and texts of this kind.
    pub fn spec(self) -> &'static NodeSpec {
        match self {
            Self::VoxelColor => &VOXEL_COLOR,
            Self::AmbientOcclusion => &AMBIENT_OCCLUSION,
            Self::Value => &VALUE,
            Self::Noise => &NOISE,
            Self::Cells => &CELLS,
            Self::Gradient => &GRADIENT,
            Self::EdgeWear => &EDGE_WEAR,
            Self::Mix => &MIX,
            Self::ColorRamp => &COLOR_RAMP,
            Self::HueShift => &HUE_SHIFT,
            Self::Remap => &REMAP,
            Self::Multiply => &MULTIPLY,
            Self::Clamp => &CLAMP,
            Self::VoxelBevel => &VOXEL_BEVEL,
            Self::SurfaceOutput => &SURFACE_OUTPUT,
            Self::VolumeOutput => &VOLUME_OUTPUT,
        }
    }

    /// Whether this kind ends the graph (it has inputs but no outputs, and at most one exists).
    pub const fn is_output(self) -> bool {
        matches!(self, Self::SurfaceOutput | Self::VolumeOutput)
    }

    /// Every kind of `category`, in library order.
    pub fn in_category(category: NodeCategory) -> impl Iterator<Item = Self> {
        Self::ALL
            .into_iter()
            .filter(move |kind| kind.spec().category == category)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_matching_spec_with_consistent_defaults() {
        for kind in GraphNodeKind::ALL {
            let spec = kind.spec();
            assert_eq!(spec.kind, kind);
            for input in spec.inputs {
                assert!(input.default.fits(input.ty), "{} / {}", spec.label, input.name);
            }
            if let Some(index) = spec.bypass_input {
                assert!(index < spec.inputs.len(), "{}", spec.label);
            }
            assert_eq!(kind.is_output(), spec.outputs.is_empty(), "{}", spec.label);
            for param in spec.params {
                assert!(
                    param.kind.sanitize(&param.kind.default_value()).is_some(),
                    "{} / {}",
                    spec.label,
                    param.name
                );
            }
        }
    }

    #[test]
    fn sanitizing_clamps_numbers_and_rejects_the_wrong_variant() {
        let kind = ParamKind::Float {
            min: 0.0,
            max: 2.0,
            default: 1.0,
        };
        assert_eq!(kind.sanitize(&ParamValue::Float(9.0)), Some(ParamValue::Float(2.0)));
        assert_eq!(
            kind.sanitize(&ParamValue::Float(f32::NAN)),
            Some(ParamValue::Float(1.0))
        );
        assert_eq!(kind.sanitize(&ParamValue::Int(1)), None);
    }

    #[test]
    fn ramp_stops_are_sorted_and_limited() {
        let stop = |position| RampStop {
            position,
            color: [0.0; 3],
        };
        let sorted = ParamKind::Stops.sanitize(&ParamValue::Stops(vec![stop(0.9), stop(0.1), stop(0.5)]));
        let stops = sorted
            .and_then(|value| match value {
                ParamValue::Stops(stops) => Some(stops),
                _ => None,
            })
            .unwrap();
        assert!(stops[0].position < stops[1].position && stops[1].position < stops[2].position);
        assert_eq!(ParamKind::Stops.sanitize(&ParamValue::Stops(vec![stop(0.0)])), None);
    }
}
