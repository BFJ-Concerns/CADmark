// Export formats the modelling kernel can write the final solid to.

use serde::{Deserialize, Serialize};

/// A file format the current model can be exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    /// STEP AP214 B-rep, the interchange format for other CAD tools.
    Step,
    /// Binary STL mesh, the common slicer input.
    Stl,
    /// 3MF mesh with units, the modern slicer input.
    ThreeMf,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 3] = [Self::Step, Self::Stl, Self::ThreeMf];

    /// File extension without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Stl => "stl",
            Self::ThreeMf => "3mf",
        }
    }

    /// Short label for buttons and menus.
    pub fn label(self) -> &'static str {
        match self {
            Self::Step => "STEP",
            Self::Stl => "STL",
            Self::ThreeMf => "3MF",
        }
    }
}
