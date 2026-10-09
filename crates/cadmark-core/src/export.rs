// Export formats the modelling kernel can write a kept model to — the solid
// formats a slicer or CAD tool reads, and the drawing formats a sketch is
// written to before it becomes a solid — and the report an export proves
// itself with: the written file read back and compared with the retained
// model, because the writer's own success status cannot be trusted (OCCT's
// STEP writer drops faces it cannot carry and reports success).

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

    /// How far the read-back file's volume and size may sit from the
    /// retained model's and still reproduce it: B-rep is exact to
    /// 0.01 %, a mesh approximates curves to 1 %. A drawing carries a
    /// profile, not a part, and has no reproduction tolerance.
    pub fn reproduction_tolerance(self) -> Option<f64> {
        match self {
            Self::Step => Some(EXACT_TOLERANCE),
            Self::Stl | Self::ThreeMf => Some(APPROXIMATE_TOLERANCE),
            Self::Svg | Self::Dxf => None,
        }
    }
}

/// Relative tolerance for a B-rep format, which carries the geometry itself.
pub const EXACT_TOLERANCE: f64 = 0.000_1;
/// Relative tolerance for a mesh, and for a STEP whose curved faces were
/// converted to B-splines to be written at all.
pub const APPROXIMATE_TOLERANCE: f64 = 0.01;

/// The counts and measures a written file is compared on. Solids and faces
/// are what a B-rep format carries; closed shells are what a mesh carries
/// (an STL has no bodies, only surfaces, so two parts in one file read back
/// as one solid of two shells, and a cracked mesh as none); volume and size
/// are common to both.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShapeFigures {
    pub solids: usize,
    /// Shells with no open boundary.
    pub shells: usize,
    pub faces: usize,
    pub volume: f64,
    pub size: [f64; 3],
}

/// A face of the retained model that no face of the read-back file matches:
/// its surface kind and the kinds of curve bounding it, which is what the
/// user or the AI needs to know which construct the format could not carry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LostFace {
    /// The kernel's surface kind name (`plane`, `cylinder`, `extrusion`, …).
    pub surface: String,
    /// The distinct curve kinds of the face's edges, sorted.
    pub curves: Vec<String>,
    pub area: f64,
}

/// The B-spline conversion a STEP export needed because the plain write did
/// not reproduce the part, and the volume shift it introduced (relative to
/// the retained model; signed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Conversion {
    pub volume_deviation: f64,
}

/// What an export proved about the file it wrote. The kernel reads the file
/// back and fills this in; every consumer — the export the user asked for,
/// the check the AI runs in a turn — reads the same report, so the two can
/// never disagree about the same part and format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportReport {
    pub format: ExportFormat,
    /// Figures of the retained model the file was written from.
    pub retained: ShapeFigures,
    /// Figures of the shape the file reads back as; `None` for a drawing
    /// format, which is written without a reproduction check.
    pub written: Option<ShapeFigures>,
    /// Faces of the retained model absent from the read-back file.
    pub lost_faces: Vec<LostFace>,
    /// Set when the STEP writer needed the B-spline conversion.
    pub conversion: Option<Conversion>,
}

/// The one-word outcome of an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportVerdict {
    /// The file reads back as the part, within the format's tolerance.
    Reproduced,
    /// The file did not reproduce the part; the kernel removed it.
    Refused,
    /// A drawing: written, and outside the reproduction check by nature.
    Unchecked,
}

impl ExportReport {
    /// A report for `format` with no figures, no lost faces and no
    /// conversion: the base tests build on, naming only what they read.
    pub fn new(format: ExportFormat) -> Self {
        Self {
            format,
            retained: ShapeFigures::default(),
            written: None,
            lost_faces: Vec::new(),
            conversion: None,
        }
    }

    /// The tolerance this report is judged at: the format's, widened to the
    /// approximate figure when a conversion changed the geometry to write it.
    pub fn tolerance(&self) -> Option<f64> {
        match (self.conversion, self.format.reproduction_tolerance()) {
            (Some(_), Some(_)) => Some(APPROXIMATE_TOLERANCE),
            (None, tolerance) => tolerance,
            (Some(_), None) => None,
        }
    }

    pub fn verdict(&self) -> ExportVerdict {
        match &self.written {
            None => ExportVerdict::Unchecked,
            Some(_) if self.discrepancies().is_empty() => ExportVerdict::Reproduced,
            Some(_) => ExportVerdict::Refused,
        }
    }

    pub fn refused(&self) -> bool {
        self.verdict() == ExportVerdict::Refused
    }

    /// Every way the read-back figures differ from the retained model's
    /// beyond tolerance, as sentences; empty when the file reproduces the
    /// part. The comparison is the format's own: solids and faces for a
    /// B-rep, shells for a mesh, volume and size for both.
    pub fn discrepancies(&self) -> Vec<String> {
        let (Some(written), Some(tolerance)) = (&self.written, self.tolerance()) else {
            return Vec::new();
        };
        let retained = &self.retained;
        let mut found = Vec::new();
        match self.format {
            ExportFormat::Step => {
                if written.solids != retained.solids {
                    found.push(format!(
                        "{} where the part has {}",
                        count(written.solids, "solid"),
                        count(retained.solids, "solid")
                    ));
                }
                if written.faces != retained.faces {
                    found.push(format!(
                        "{} where the part has {}",
                        count(written.faces, "face"),
                        count(retained.faces, "face")
                    ));
                }
            }
            ExportFormat::Stl | ExportFormat::ThreeMf => {
                if written.shells != retained.shells {
                    found.push(format!(
                        "{} where the part has {}",
                        count(written.shells, "closed surface"),
                        count(retained.shells, "closed surface")
                    ));
                }
            }
            ExportFormat::Svg | ExportFormat::Dxf => {}
        }
        if !within(written.volume, retained.volume, tolerance) {
            found.push(format!(
                "volume {} mm³ where the part measures {} mm³",
                compact(written.volume),
                compact(retained.volume)
            ));
        }
        let size_differs = written
            .size
            .iter()
            .zip(retained.size)
            .any(|(actual, expected)| !within(*actual, expected, tolerance));
        if size_differs {
            found.push(format!(
                "size {} where the part measures {}",
                dimensions(written.size),
                dimensions(retained.size)
            ));
        }
        found
    }

    /// The cause a consumer states alongside its own verdict sentence: the
    /// conversion and its deviation for a reproduced STEP that needed one;
    /// the discrepancies and the faces lost for a refusal; nothing for a
    /// plain success or a drawing. Both the export the user asked for and
    /// the AI's in-turn check carry this text, so they name the same cause.
    pub fn explain(&self) -> String {
        let mut sentences = Vec::new();
        let discrepancies = self.discrepancies();
        if !discrepancies.is_empty() {
            sentences.push(format!(
                "The file read back with {}.",
                join_clauses(&discrepancies)
            ));
        }
        if !self.lost_faces.is_empty() {
            sentences.push(format!(
                "Faces missing from the file: {}.",
                describe_lost_faces(&self.lost_faces)
            ));
        }
        if let Some(conversion) = self.conversion {
            sentences.push(match self.verdict() {
                ExportVerdict::Refused => "The STEP writer cannot carry faces built on offset \
                                           curves; converting them to B-splines did not recover \
                                           the file either."
                    .to_string(),
                _ => format!(
                    "The STEP writer cannot carry faces built on offset curves, so they were \
                     converted to B-splines before writing; the conversion shifted the volume \
                     by {}.",
                    percentage(conversion.volume_deviation)
                ),
            });
        }
        sentences.join(" ")
    }
}

/// The length below which two positions are the same point to the kernel
/// (OCCT's confusion precision, in millimetres). It floors the relative
/// comparisons so a zero expectation — a sketch's volume, a flat profile's
/// thin axis — tolerates floating-point noise without a sub-millimetre
/// feature escaping its percentage.
pub const GEOMETRIC_CONFUSION: f64 = 1e-7;

/// Whether `actual` sits within `tolerance` of `expected`, relatively.
fn within(actual: f64, expected: f64, tolerance: f64) -> bool {
    (actual - expected).abs() <= tolerance * expected.abs().max(GEOMETRIC_CONFUSION)
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn compact(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn dimensions(size: [f64; 3]) -> String {
    format!(
        "{} × {} × {} mm",
        compact(size[0]),
        compact(size[1]),
        compact(size[2])
    )
}

fn percentage(relative: f64) -> String {
    let percent = relative * 100.0;
    if percent.abs() < 0.0001 {
        "less than 0.0001 %".to_string()
    } else {
        format!(
            "{} %",
            format!("{percent:.4}")
                .trim_end_matches('0')
                .trim_end_matches('.')
        )
    }
}

fn join_clauses(clauses: &[String]) -> String {
    match clauses {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{}, and {last}", head.join(", ")),
    }
}

/// Lost faces grouped by kind, so eleven extrusions over offset curves
/// read as one clause rather than eleven.
fn describe_lost_faces(faces: &[LostFace]) -> String {
    let mut groups: Vec<(&LostFace, usize)> = Vec::new();
    for face in faces {
        match groups
            .iter_mut()
            .find(|(seen, _)| seen.surface == face.surface && seen.curves == face.curves)
        {
            Some((_, n)) => *n += 1,
            None => groups.push((face, 1)),
        }
    }
    let clauses: Vec<String> = groups
        .iter()
        .map(|(face, n)| {
            let curves = if face.curves.is_empty() {
                String::new()
            } else {
                format!(" bounded by {} curves", join_clauses(&face.curves))
            };
            format!(
                "{} on {}{curves}",
                count(*n, "face"),
                surface_noun(&face.surface, *n)
            )
        })
        .collect();
    join_clauses(&clauses)
}

/// The surface kind as a noun phrase: `a plane`, `an extrusion surface`,
/// `extrusion surfaces`.
fn surface_noun(surface: &str, n: usize) -> String {
    let noun = match surface {
        "extrusion" => "extrusion surface",
        "revolution" => "surface of revolution",
        "offset" => "offset surface",
        "bspline" => "B-spline surface",
        "bezier" => "Bézier surface",
        other => other,
    };
    if n != 1 {
        format!("{noun}s")
    } else if noun.starts_with(['a', 'e', 'i', 'o', 'u']) {
        format!("an {noun}")
    } else {
        format!("a {noun}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-solid, seven-face part and a read-back that matches it exactly.
    fn faithful_step_report() -> ExportReport {
        let figures = ShapeFigures {
            solids: 1,
            shells: 1,
            faces: 7,
            volume: 10_885.8,
            size: [46.0, 30.0, 10.0],
            ..ShapeFigures::default()
        };
        ExportReport {
            retained: figures.clone(),
            written: Some(figures),
            ..ExportReport::new(ExportFormat::Step)
        }
    }

    #[test]
    fn a_file_that_reads_back_as_the_part_is_reproduced_with_nothing_to_explain() {
        let report = faithful_step_report();
        assert_eq!(report.verdict(), ExportVerdict::Reproduced);
        assert!(!report.refused());
        assert_eq!(report.explain(), "");
    }

    #[test]
    fn a_step_that_loses_faces_is_refused_and_names_what_was_lost() {
        let mut report = faithful_step_report();
        report.written = Some(ShapeFigures::default());
        report.lost_faces = vec![
            LostFace {
                surface: "extrusion".to_string(),
                curves: vec!["line".to_string(), "offset".to_string()],
                ..LostFace::default()
            };
            5
        ];
        report.lost_faces.push(LostFace {
            surface: "plane".to_string(),
            curves: vec!["offset".to_string()],
            ..LostFace::default()
        });
        assert_eq!(report.verdict(), ExportVerdict::Refused);
        let explanation = report.explain();
        assert_eq!(
            explanation,
            "The file read back with 0 solids where the part has 1 solid, 0 faces where the \
             part has 7 faces, volume 0 mm³ where the part measures 10885.8 mm³, and size \
             0 × 0 × 0 mm where the part measures 46 × 30 × 10 mm. Faces missing from the \
             file: 5 faces on extrusion surfaces bounded by line, and offset curves, and 1 \
             face on a plane bounded by offset curves."
        );
    }

    #[test]
    fn a_converted_step_is_judged_at_the_approximate_tolerance_and_states_its_deviation() {
        let mut report = faithful_step_report();
        let written = report.written.as_mut().unwrap();
        written.volume *= 1.0 + 0.001; // 0.1 %: past the exact figure, within the approximate one
        assert_eq!(report.verdict(), ExportVerdict::Refused);
        report.conversion = Some(Conversion {
            volume_deviation: 0.001,
        });
        assert_eq!(report.tolerance(), Some(APPROXIMATE_TOLERANCE));
        assert_eq!(report.verdict(), ExportVerdict::Reproduced);
        assert_eq!(
            report.explain(),
            "The STEP writer cannot carry faces built on offset curves, so they were converted \
             to B-splines before writing; the conversion shifted the volume by 0.1 %."
        );
    }

    #[test]
    fn a_mesh_is_compared_on_shells_not_solids_and_a_drawing_is_unchecked() {
        let mut report = ExportReport {
            retained: ShapeFigures {
                solids: 2,
                shells: 2,
                faces: 12,
                volume: 1125.0,
                size: [35.0, 10.0, 10.0],
                ..ShapeFigures::default()
            },
            // An STL of two bodies reads back as one solid of two shells.
            written: Some(ShapeFigures {
                solids: 1,
                shells: 2,
                faces: 24,
                volume: 1125.0,
                size: [35.0, 10.0, 10.0],
                ..ShapeFigures::default()
            }),
            ..ExportReport::new(ExportFormat::Stl)
        };
        assert_eq!(report.verdict(), ExportVerdict::Reproduced);
        report.written.as_mut().unwrap().shells = 1;
        assert_eq!(
            report.discrepancies(),
            ["1 closed surface where the part has 2 closed surfaces"]
        );

        let drawing = ExportReport::new(ExportFormat::Svg);
        assert_eq!(drawing.verdict(), ExportVerdict::Unchecked);
        assert_eq!(drawing.explain(), "");
    }

    #[test]
    fn a_zero_expectation_tolerates_floating_point_noise_but_not_a_real_difference() {
        assert!(within(1e-12, 0.0, EXACT_TOLERANCE));
        assert!(!within(0.5, 0.0, EXACT_TOLERANCE));
        assert!(within(10_001.0, 10_000.0, EXACT_TOLERANCE));
        assert!(!within(10_002.0, 10_000.0, EXACT_TOLERANCE));
        // A sub-millimetre feature is held to its percentage, not to a unit.
        assert!(!within(0.109, 0.1, APPROXIMATE_TOLERANCE));
        assert!(within(0.1005, 0.1, APPROXIMATE_TOLERANCE));
    }

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
