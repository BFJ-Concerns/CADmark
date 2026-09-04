// Status bar — what the worker is doing, else the last result, plus the
// model's measurements and the navigation hint.

use cadmark_core::geometry::{GeometryDescriptors, ModelSummary, TopologyElement, compact};

use crate::theme;

/// A line for the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub is_error: bool,
}

impl Status {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Everything the status bar reads.
pub struct StatusView<'a> {
    /// What the worker is doing, if anything.
    pub activity: Option<&'a str>,
    /// The last outcome to report.
    pub status: Option<&'a Status>,
    /// Measurements of the model on screen.
    pub summary: Option<&'a ModelSummary>,
    /// What the user has selected, as a short label.
    pub selection: Option<String>,
    /// The measurement for the current selection, if it can be read locally.
    pub measurement: Option<&'a str>,
}

/// Describe the measurement available from one picked element's descriptors.
pub fn selection_measurement(
    element: &TopologyElement,
    descriptors: &GeometryDescriptors,
) -> Option<String> {
    match element {
        TopologyElement::Face(id) => descriptors
            .face(*id)
            .map(|face| format!("Area {} mm²", compact(face.area))),
        TopologyElement::Edge(id) => descriptors.edge(*id).map(|edge| {
            if let Some(radius) = edge.radius {
                format!("Diameter {} mm", compact(radius * 2.0))
            } else {
                format!("Length {} mm", compact(edge.length))
            }
        }),
        // A part's own measurements are its summary, shown elsewhere; the
        // status bar reports what a picked element within one measures.
        TopologyElement::Part(_) | TopologyElement::Vertex(_) => None,
    }
}

/// Render the status bar contents.
pub fn show_status_bar(ui: &mut egui::Ui, view: StatusView<'_>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if let Some(activity) = view.activity {
            ui.add(egui::Spinner::new().size(12.0).color(theme::ACCENT));
            ui.label(egui::RichText::new(activity).color(theme::TEXT));
        } else if let Some(status) = view.status {
            let (dot, colour) = if status.is_error {
                ("\u{25CF}", theme::ERROR)
            } else {
                ("\u{25CF}", theme::SUCCESS)
            };
            ui.label(egui::RichText::new(dot).color(colour).small());
            ui.add(
                egui::Label::new(egui::RichText::new(&status.text).color(if status.is_error {
                    theme::ERROR
                } else {
                    theme::TEXT
                }))
                .truncate(),
            )
            .on_hover_text(&status.text);
        } else {
            ui.label(egui::RichText::new("Ready").color(theme::TEXT_MUTED));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(
                    "Right-drag orbit · Middle-drag pan · Scroll zoom · Click to comment",
                )
                .small()
                .color(theme::TEXT_MUTED),
            );
            if let Some(summary) = view.summary {
                ui.separator();
                ui.label(
                    egui::RichText::new(summary.describe())
                        .small()
                        .color(theme::TEXT_MUTED),
                )
                .on_hover_text("Faces, volume and bounding box of the model on screen");
            }
            if let Some(selection) = view.selection {
                ui.separator();
                theme::chip(ui, &selection, theme::SPATIAL);
            }
            if let Some(measurement) = view.measurement {
                ui.separator();
                theme::chip(ui, measurement, theme::SUCCESS);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use cadmark_core::geometry::{
        EdgeDescriptor, EdgeId, FaceDescriptor, FaceId, GeometryDescriptors, TopologyElement,
    };

    use super::selection_measurement;

    #[test]
    fn reads_face_area_and_circular_edge_diameter_from_descriptors() {
        let descriptors = GeometryDescriptors {
            faces: vec![FaceDescriptor {
                surface_type: "plane".into(),
                area: 250.0,
                centre: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
                neighbours: vec![],
            }],
            edges: vec![EdgeDescriptor {
                curve_type: "circle".into(),
                length: std::f64::consts::PI * 12.0,
                radius: Some(6.0),
                centre: [0.0; 3],
                neighbours: vec![],
            }],
            vertices: vec![],
        };
        assert_eq!(
            selection_measurement(&TopologyElement::Face(FaceId(0)), &descriptors),
            Some("Area 250 mm²".into())
        );
        assert_eq!(
            selection_measurement(&TopologyElement::Edge(EdgeId(0)), &descriptors),
            Some("Diameter 12 mm".into())
        );
    }

    #[test]
    fn reads_a_circular_arc_diameter_from_its_radius_not_its_arc_length() {
        let descriptors = GeometryDescriptors {
            faces: vec![],
            edges: vec![EdgeDescriptor {
                curve_type: "circle".into(),
                length: std::f64::consts::PI * 1.5,
                radius: Some(3.0),
                centre: [0.0; 3],
                neighbours: vec![],
            }],
            vertices: vec![],
        };
        assert_eq!(
            selection_measurement(&TopologyElement::Edge(EdgeId(0)), &descriptors),
            Some("Diameter 6 mm".into())
        );
    }
}
