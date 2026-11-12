//! Stroke icons from the mockups, drawn from their SVG path data (24 × 24 view box, 1.75 stroke).
//!
//! ponytail: a tiny path parser (M L H V C A Z, absolute and relative) flattened into polylines instead of an SVG
//! rendering dependency; extend the parser if a new icon needs other commands.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

/// Every icon the HUD uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Menu,
    Undo,
    Redo,
    Grid,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Minimize,
    Maximize,
    Close,
    SelectBox,
    SelectColor,
    Attach,
    Erase,
    Paint,
    Fill,
    Pick,
    Move,
    Measure,
    BrushVoxel,
    BrushLine,
    BrushBox,
    BrushFace,
    Palette,
    Cube,
    Folder,
    Search,
    Settings,
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Orbit,
    Dolly,
    Focus,
    Sun,
    Play,
    Pause,
    Plus,
    Guides,
    Pointer,
    AddNode,
    Fit,
    Hand,
    Grab,
    Forbidden,
    Scissors,
}

impl Icon {
    fn path(self) -> &'static str {
        match self {
            Self::Menu => "M4 7h16 M4 12h16 M4 17h16",
            Self::Undo => "M8 5L3 10l5 5 M3 10h11a5 5 0 0 1 0 10h-4",
            Self::Redo => "M16 5l5 5-5 5 M21 10H10a5 5 0 0 0 0 10h4",
            Self::Grid | Self::BrushFace => "M4 4h16v16H4z M4 12h16 M12 4v16",
            Self::ChevronDown => "M6 9l6 6 6-6",
            Self::ChevronLeft => "M15 6l-6 6 6 6",
            Self::ChevronRight => "M9 6l6 6-6 6",
            Self::Minimize => "M6 12h12",
            Self::Maximize => "M6 6h12v12H6z",
            Self::Close => "M6 6l12 12 M18 6L6 18",
            Self::SelectBox => "M4 4h4 M12 4h4 M20 4v4 M20 12v4 M20 20h-4 M12 20H8 M4 20v-4 M4 12V8",
            Self::SelectColor => "M4 20L14 10 M13 7l4 4 M16 3v3 M14.5 4.5h3 M20 8v3 M18.5 9.5h3",
            Self::Attach => "M11 6l7 4v8l-7 4-7-4v-8z M11 14l7-4 M11 14v8 M11 14l-7-4 M19 2v6 M16 5h6",
            Self::Erase => "M3 17l10-10 7 7-6 6H9z M9 11l7 7 M14 20h7",
            Self::Paint => "M18 3l3 3-9 9-3-3z M8 13c-3 0-4 2-4 4 0 1-1 2-2 2 3 2 9 1 9-3",
            Self::Fill => "M5 12l7-7 8 8-7 7a2 2 0 0 1-3 0l-5-5a2 2 0 0 1 0-3z M4 13h15",
            Self::Pick => "M12 8l4 4-8 8H4v-4z M15 5a2.5 2.5 0 0 1 4 4l-2 2-4-4z",
            Self::Move => {
                "M12 3v18 M3 12h18 M12 3l-3 3 M12 3l3 3 M12 21l-3-3 M12 21l3-3 M3 12l3-3 M3 12l3 3 M21 12l-3-3 M21 12l-3 3"
            }
            Self::Measure => "M3 16L16 3l5 5L8 21z M7 12l2 2 M10 9l2 2 M13 6l2 2",
            Self::BrushVoxel => "M8 8h8v8H8z",
            Self::BrushLine => "M5 19L19 5",
            Self::BrushBox => "M4 8h12v12H4z M4 8l4-4h12l-4 4 M20 4v12l-4 4",
            Self::Palette => "M4 4h7v7H4z M13 4h7v7h-7z M4 13h7v7H4z M13 13h7v7h-7z",
            Self::Cube => "M12 3l8 4.5v9L12 21l-8-4.5v-9z M12 12l8-4.5 M12 12v9 M12 12L4 7.5",
            Self::Folder => "M3 6h6l2 2h10v11H3z",
            Self::Search => "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z M16 16l4 4",
            Self::Settings => "M4 7h10 M18 7h2 M4 17h2 M10 17h10 M16 5v4 M8 15v4",
            Self::Eye => {
                "M2 12c3-5 6-7 10-7c4 0 7 2 10 7c-3 5-6 7-10 7c-4 0-7-2-10-7z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z"
            }
            Self::EyeOff => {
                "M2 12c3-5 6-7 10-7c4 0 7 2 10 7c-3 5-6 7-10 7c-4 0-7-2-10-7z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z M4 4l16 16"
            }
            Self::Lock => "M6 11h12v9H6z M8 11V8a4 4 0 0 1 8 0v3",
            Self::Orbit => "M20 12a8 8 0 1 1-3-6.2 M20 4v5h-5",
            Self::Dolly => "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z M16 16l4 4 M11 8v6 M8 11h6",
            Self::Focus => "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z M12 2v4 M12 18v4 M2 12h4 M18 12h4",
            Self::Sun => "M8 4a4 4 0 1 0 0 8 4 4 0 0 0 0-8z M13 13l7 7 M20 15v5h-5",
            Self::Play => "M7 4l13 8-13 8z",
            Self::Pause => "M8 5v14 M16 5v14",
            Self::Plus => "M12 5v14 M5 12h14",
            Self::Guides => "M4 4h16v16H4z M4 9.5h16 M4 14.5h16 M9.5 4v16 M14.5 4v16",
            Self::Pointer => "M5 3l14 8-6 2-2 6z",
            Self::AddNode => "M4 4h16v16H4z M12 8v8 M8 12h8",
            Self::Fit => "M4 9V4h5 M15 4h5v5 M20 15v5h-5 M9 20H4v-5",
            Self::Hand => {
                "M8 13V6a1.5 1.5 0 0 1 3 0v5 M11 11V4.5a1.5 1.5 0 0 1 3 0V11 M14 11V6a1.5 1.5 0 0 1 3 0v8c0 4-2 6-5 6h-1c-2 0-3-1-4-3l-2-4a1.5 1.5 0 0 1 2.5-1.5L8 14"
            }
            Self::Grab => {
                "M7 13v-2a1.5 1.5 0 0 1 3 0 M10 11v-1.5a1.5 1.5 0 0 1 3 0 M13 10v-.5a1.5 1.5 0 0 1 3 0V14c0 4-2 6-5 6h-1c-2 0-3-1-4-3l-1-3a1.5 1.5 0 0 1 2.5-1.5L7 13"
            }
            Self::Forbidden => "M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16z M6.3 6.3l11.4 11.4",
            Self::Scissors => {
                "M6 4a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z M6 15a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z M8 8l12 9 M8 16L20 7"
            }
            Self::Unlock => "M6 11h12v9H6z M8 11V8a4 4 0 0 1 7.5-2",
        }
    }

    /// Paints the icon centred in `rect`, scaled so the 24-unit view box fills it.
    pub(crate) fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        self.stroke(painter, rect, color, 0.0);
    }

    /// Like [`Icon::paint`], with a dark outline one point wide on each side so the icon reads on any background.
    pub(crate) fn paint_outlined(self, painter: &Painter, rect: Rect, color: Color32, outline: Color32) {
        self.stroke(painter, rect, outline, 2.0);
        self.stroke(painter, rect, color, 0.0);
    }

    fn stroke(self, painter: &Painter, rect: Rect, color: Color32, extra_width: f32) {
        let scale = rect.width().min(rect.height()) / 24.0;
        let origin = rect.center() - vec2(12.0, 12.0) * scale;
        let stroke = Stroke::new((1.75 * scale).max(1.0) + extra_width, color);
        for polyline in flatten(self.path()) {
            let points: Vec<Pos2> = polyline.iter().map(|point| origin + point.to_vec2() * scale).collect();
            if points.len() >= 2 {
                painter.add(Shape::line(points, stroke));
            }
        }
    }
}

/// Turns SVG path data into polylines in view-box units.
fn flatten(path: &str) -> Vec<Vec<Pos2>> {
    let mut tokens = Tokens::new(path);
    let mut lines: Vec<Vec<Pos2>> = Vec::new();
    let mut current = Vec::new();
    let mut position = Pos2::ZERO;
    let mut start = Pos2::ZERO;
    let mut command = 'M';
    while let Some(next) = tokens.command_or_repeat(command) {
        command = next;
        let relative = command.is_ascii_lowercase();
        let base = if relative { position.to_vec2() } else { Vec2::ZERO };
        match command.to_ascii_uppercase() {
            'M' => {
                let Some(point) = tokens.point() else { break };
                if current.len() > 1 {
                    lines.push(std::mem::take(&mut current));
                }
                current.clear();
                position = point + base;
                start = position;
                current.push(position);
                // Further coordinate pairs after a move are implicit line-tos.
                command = if relative { 'l' } else { 'L' };
            }
            'L' => {
                let Some(point) = tokens.point() else { break };
                position = point + base;
                current.push(position);
            }
            'H' => {
                let Some(x) = tokens.number() else { break };
                position.x = if relative { position.x + x } else { x };
                current.push(position);
            }
            'V' => {
                let Some(y) = tokens.number() else { break };
                position.y = if relative { position.y + y } else { y };
                current.push(position);
            }
            'C' => {
                let (Some(c1), Some(c2), Some(end)) = (tokens.point(), tokens.point(), tokens.point()) else {
                    break;
                };
                let (c1, c2, end) = (c1 + base, c2 + base, end + base);
                for step in 1..=12 {
                    let t = step as f32 / 12.0;
                    let u = 1.0 - t;
                    let point = position.to_vec2() * (u * u * u)
                        + c1.to_vec2() * (3.0 * u * u * t)
                        + c2.to_vec2() * (3.0 * u * t * t)
                        + end.to_vec2() * (t * t * t);
                    current.push(point.to_pos2());
                }
                position = end;
            }
            'A' => {
                let values: Option<Vec<f32>> = (0..7).map(|_| tokens.number()).collect();
                let Some(values) = values else { break };
                let end = pos2(values[5], values[6]) + base;
                let flags = ArcFlags {
                    large: values[3] != 0.0,
                    sweep: values[4] != 0.0,
                };
                arc(&mut current, position, end, vec2(values[0], values[1]), flags);
                position = end;
            }
            'Z' => {
                current.push(start);
                position = start;
                lines.push(std::mem::take(&mut current));
                current.push(position);
                command = 'M';
                if !tokens.has_command() {
                    break;
                }
            }
            _ => break,
        }
    }
    if current.len() > 1 {
        lines.push(current);
    }
    lines
}

/// Appends an SVG elliptical arc (endpoint parameterisation, no rotation) as line segments.
/// The two SVG arc flags: take the long way round, and sweep in the positive-angle direction.
#[derive(Debug, Clone, Copy)]
struct ArcFlags {
    large: bool,
    sweep: bool,
}

fn arc(out: &mut Vec<Pos2>, from: Pos2, to: Pos2, radii: Vec2, flags: ArcFlags) {
    let ArcFlags { large, sweep } = flags;
    let half = (from - to) / 2.0;
    let mut radii = radii.abs();
    let lambda = (half.x / radii.x).powi(2) + (half.y / radii.y).powi(2);
    if lambda > 1.0 {
        radii *= lambda.sqrt();
    }
    let (rx, ry) = (radii.x, radii.y);
    let numerator = (rx * rx * ry * ry - rx * rx * half.y * half.y - ry * ry * half.x * half.x).max(0.0);
    let denominator = rx * rx * half.y * half.y + ry * ry * half.x * half.x;
    let mut factor = if denominator > 0.0 {
        (numerator / denominator).sqrt()
    } else {
        0.0
    };
    if large == sweep {
        factor = -factor;
    }
    let center_prime = vec2(factor * rx * half.y / ry, -factor * ry * half.x / rx);
    let center = center_prime + (from.to_vec2() + to.to_vec2()) / 2.0;
    let angle = |v: Vec2| v.y.atan2(v.x);
    let start = angle(vec2((half.x - center_prime.x) / rx, (half.y - center_prime.y) / ry));
    let mut delta = angle(vec2((-half.x - center_prime.x) / rx, (-half.y - center_prime.y) / ry)) - start;
    if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    } else if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    }
    for step in 1..=16 {
        let theta = start + delta * step as f32 / 16.0;
        out.push((center + vec2(rx * theta.cos(), ry * theta.sin())).to_pos2());
    }
}

struct Tokens<'path> {
    rest: &'path str,
}

impl<'path> Tokens<'path> {
    fn new(path: &'path str) -> Self {
        Self { rest: path }
    }

    fn skip_separators(&mut self) {
        self.rest = self.rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
    }

    fn has_command(&mut self) -> bool {
        self.skip_separators();
        self.rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    }

    /// The next command letter, or `repeat` when the path continues with bare numbers.
    fn command_or_repeat(&mut self, repeat: char) -> Option<char> {
        self.skip_separators();
        let next = self.rest.chars().next()?;
        if next.is_ascii_alphabetic() {
            self.rest = &self.rest[1..];
            Some(next)
        } else {
            Some(repeat)
        }
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_separators();
        let mut end = 0;
        let mut seen_dot = false;
        for (index, c) in self.rest.char_indices() {
            let ok = c.is_ascii_digit() || (c == '-' && index == 0) || (c == '.' && !seen_dot);
            if !ok {
                break;
            }
            seen_dot |= c == '.';
            end = index + c.len_utf8();
        }
        let value = self.rest[..end].parse().ok()?;
        self.rest = &self.rest[end..];
        Some(value)
    }

    fn point(&mut self) -> Option<Pos2> {
        Some(pos2(self.number()?, self.number()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_lines_and_close_produce_a_closed_square() {
        let lines = flatten("M8 8h8v8H8z");
        assert_eq!(
            lines[0],
            vec![
                pos2(8.0, 8.0),
                pos2(16.0, 8.0),
                pos2(16.0, 16.0),
                pos2(8.0, 16.0),
                pos2(8.0, 8.0)
            ]
        );
    }

    #[test]
    fn implicit_line_tos_follow_a_move() {
        let lines = flatten("M6 9l6 6 6-6");
        assert_eq!(lines[0], vec![pos2(6.0, 9.0), pos2(12.0, 15.0), pos2(18.0, 9.0)]);
    }

    #[test]
    fn arcs_end_where_the_path_says() {
        let lines = flatten("M3 10h11a5 5 0 0 1 0 10h-4");
        let last_arc_point = lines[0][lines[0].len() - 2];
        assert!((last_arc_point - pos2(14.0, 20.0)).length() < 1e-3);
    }
}
