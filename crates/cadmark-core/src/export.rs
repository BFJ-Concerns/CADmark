// Export formats the modelling kernel can write a kept model to: the solid
// formats a slicer or CAD tool reads, and the drawing formats a sketch is
// written to before it becomes a solid.

use serde::{Deserialize, Serialize};

/// A file format the current model can be exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    /// STEP AP214 B-rep, the interchange format for other CAD tools. A
    /// sketch exports as its planar faces and curves; a solid as itself.
    Step,
    /// Binary STL mesh, the common slicer input.
    Stl,
    /// 3MF mesh with units, the modern slicer input.
    ThreeMf,
    /// SVG drawing of a sketch in its own plane, in millimetres.
    Svg,
    /// DXF drawing of a sketch in its own plane, for 2D CAD and cutters.
    Dxf,
}

impl ExportFormat {
    /// What a solid can be written to.
    pub const SOLID: [ExportFormat; 3] = [Self::Step, Self::Stl, Self::ThreeMf];
    /// What a sketch can be written to: flat drawings of its plane, and
    /// STEP carrying its faces and curves to another CAD tool.
    pub const SKETCH: [ExportFormat; 3] = [Self::Svg, Self::Dxf, Self::Step];

    /// File extension without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Stl => "stl",
            Self::ThreeMf => "3mf",
            Self::Svg => "svg",
            Self::Dxf => "dxf",
        }
    }

    /// Short label for buttons and menus.
    pub fn label(self) -> &'static str {
        match self {
            Self::Step => "STEP",
            Self::Stl => "STL",
            Self::ThreeMf => "3MF",
            Self::Svg => "SVG",
            Self::Dxf => "DXF",
        }
    }

    /// Whether the format is a flat drawing, which needs the plane a
    /// sketch was drawn on and cannot carry a solid.
    pub fn is_drawing(self) -> bool {
        matches!(self, Self::Svg | Self::Dxf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawing_formats_are_offered_to_sketches_and_never_to_solids() {
        for format in ExportFormat::SOLID {
            assert!(
                !format.is_drawing(),
                "{} is not a solid format",
                format.label()
            );
        }
        assert!(ExportFormat::SKETCH.contains(&ExportFormat::Step));
        assert!(
            ExportFormat::SKETCH
                .iter()
                .any(|format| format.is_drawing())
        );
        assert!(!ExportFormat::SKETCH.contains(&ExportFormat::Stl));
    }
}
