// Export of a kept model to interchange, slicer and drawing formats, proven
// by reading the written file back.
//
// Runs inside the kernel worker: the model is the BREP file an execution
// left in the worker's scratch directory, re-read here so an export never
// depends on the script running again. A solid format is written, read
// back, and compared with the retained model — solids and faces for STEP,
// closed shells for a mesh, volume and size for both — because the
// writer's own status cannot be trusted: OCCT's STEP writer omits faces
// built on offset curves and reports success. When a plain STEP write
// loses geometry, the faces the writer cannot carry are converted to
// B-splines and the file written again; the report says so and states the
// volume the conversion moved. A file that still does not reproduce the
// part is removed, so nothing beside the script claims a success that did
// not happen. A drawing format is written in the plane the sketch was
// drawn on, so the file shows the profile as the viewport does, at true
// size in millimetres; drawings carry a profile, not a part, and are
// written without a reproduction check.

use std::path::Path;

use cadmark_core::export::{
    Conversion, ExportFormat, ExportReport, GEOMETRIC_CONFUSION, LostFace, ShapeFigures,
};
use cadmark_core::sketch::SketchPlane;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use thiserror::Error;

use crate::protocol::ModelFile;

#[derive(Error, Debug)]
pub enum ExportError {
    #[error("export failed: {0}")]
    Python(#[from] PyErr),
    #[error("the exporter reported failure writing {0}")]
    WriteFailed(String),
    #[error("a {0} drawing needs the plane the sketch was drawn on")]
    NoPlane(&'static str),
    #[error("could not remove the refused export {0}: {1}")]
    RemoveFailed(String, std::io::Error),
}

/// Defined on top of the measurement namespace, whose surface and curve
/// name tables, `_explore` and `_cadmark_exact_bounds` this source uses.
const EXPORT_SOURCE: &std::ffi::CStr = c"
from build123d import (
    Compound, ExportDXF, ExportSVG, Mesher, Plane, export_step, export_stl, import_brep,
    import_step,
)
from OCP.GeomAbs import GeomAbs_Shape
from OCP.ShapeCustom import ShapeCustom, ShapeCustom_RestrictionParameters
from OCP.ShapeFix import ShapeFix_Shape

# How close a read-back face must sit to a retained face to count as the
# same face: area to a thousandth, centre to a thousandth of the model's
# diagonal. Wide enough to absorb a B-spline conversion, far too narrow
# to pair two distinct faces of one part.
_FACE_MATCH_RELATIVE = 1e-3


def _cadmark_flatten(shape, plane):
    origin, x_axis, normal = plane
    return Plane(origin=origin, x_dir=x_axis, z_dir=normal).to_local_coords(shape)


def export_drawing(model_path, format_name, path, plane):
    shape = import_brep(model_path)
    drawing = ExportSVG() if format_name == 'svg' else ExportDXF()
    drawing.add_shape(_cadmark_flatten(shape, plane))
    drawing.write(path)
    return True


def _cadmark_write(shape, format_name, path):
    if format_name == 'step':
        return export_step(shape, path)
    if format_name == 'stl':
        return export_stl(shape, path)
    if format_name == '3mf':
        mesher = Mesher()
        mesher.add_shape(shape)
        mesher.write(path)
        return True
    raise ValueError(f'unknown export format {format_name}')


def _cadmark_read_back(format_name, path):
    if format_name == 'step':
        return import_step(path).wrapped
    return Compound(Mesher().read(path)).wrapped


def _cadmark_figures(shape):
    props = GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, props)
    bounds = _cadmark_exact_bounds(shape)
    if bounds.IsVoid():
        size = [0.0, 0.0, 0.0]
    else:
        x_min, y_min, z_min, x_max, y_max, z_max = bounds.Get()
        size = [x_max - x_min, y_max - y_min, z_max - z_min]
    return {
        'solids': sum(1 for _ in _explore(shape, TopAbs_SOLID)),
        'shells': sum(
            1 for shell in _explore(shape, TopAbs_SHELL)
            if BRep_Tool.IsClosed_s(TopoDS.Shell_s(shell))
        ),
        'faces': sum(1 for _ in _explore(shape, TopAbs_FACE)),
        'volume': props.Mass(),
        'size': size,
    }


def _cadmark_face_key(face):
    face = TopoDS.Face_s(face)
    props = GProp_GProps()
    BRepGProp.SurfaceProperties_s(face, props)
    curves = sorted({
        _curve_names.get(BRepAdaptor_Curve(TopoDS.Edge_s(edge)).GetType(), 'other')
        for edge in _explore(face, TopAbs_EDGE)
    })
    return {
        'surface': _surface_names.get(BRepAdaptor_Surface(face).GetType(), 'other'),
        'curves': curves,
        'area': props.Mass(),
        'centre': _point(props.CentreOfMass()),
    }


def _cadmark_lost_faces(retained, written):
    # Each retained face is paired with the nearest unpaired read-back face
    # by area and centre; a retained face left unpaired is one the format
    # dropped. Surface kinds are not compared: a converted face comes back
    # as a B-spline and is still the same face.
    diagonal = max(_cadmark_figures(retained)['size'] + [1.0])
    tolerance = _FACE_MATCH_RELATIVE * diagonal
    candidates = [_cadmark_face_key(face) for face in _explore(written, TopAbs_FACE)]
    lost = []
    for face in (_cadmark_face_key(face) for face in _explore(retained, TopAbs_FACE)):
        match = None
        for index, candidate in enumerate(candidates):
            if abs(candidate['area'] - face['area']) > _FACE_MATCH_RELATIVE * max(face['area'], 1.0):
                continue
            if any(abs(a - b) > tolerance for a, b in zip(candidate['centre'], face['centre'])):
                continue
            match = index
            break
        if match is None:
            lost.append({'surface': face['surface'], 'curves': face['curves'], 'area': face['area']})
        else:
            del candidates[match]
    return lost


def _cadmark_convert(shape):
    # Only what the STEP writer cannot carry is rewritten: offset curves
    # and the extrusion and offset surfaces built on them. Planes, conics
    # and ordinary B-splines pass through untouched, so a part that needed
    # no conversion would be unchanged by this.
    parameters = ShapeCustom_RestrictionParameters()
    for name in dir(parameters):
        if name.startswith('Convert'):
            setattr(parameters, name, False)
    parameters.ConvertOffsetCurv3d = True
    parameters.ConvertOffsetCurv2d = True
    parameters.ConvertOffsetSurf = True
    parameters.ConvertExtrusionSurf = True
    converted = ShapeCustom.BSplineRestriction_s(
        shape, 1e-4, 1e-6, 15, 200, GeomAbs_Shape.GeomAbs_C1, GeomAbs_Shape.GeomAbs_C1,
        True, True, parameters,
    )
    fixer = ShapeFix_Shape(converted)
    fixer.Perform()
    return Compound.cast(fixer.Shape())


def export_solid(model_path, format_name, path, convert):
    shape = import_brep(model_path)
    retained = shape.wrapped
    to_write = _cadmark_convert(retained) if convert else shape
    if not _cadmark_write(to_write, format_name, path):
        return None
    written = _cadmark_read_back(format_name, path)
    return {
        'retained': _cadmark_figures(retained),
        'written': _cadmark_figures(written),
        'lost_faces': _cadmark_lost_faces(retained, written) if format_name == 'step' else [],
    }
";

/// Write the model to `path` in the given format and prove the result. A
/// drawing format needs `plane` and is written without a check; a solid
/// format is read back and compared, converted and rewritten when the STEP
/// writer lost geometry, and removed again when the file still does not
/// reproduce the part. The returned report carries the verdict and its
/// cause; `Err` is reserved for the export not happening at all.
///
/// Takes the kernel execution lock: build123d's exporters construct OCP
/// builders, which must not run while another execution has them wrapped.
pub fn export_model(
    model: &ModelFile,
    format: ExportFormat,
    path: &Path,
    plane: Option<SketchPlane>,
) -> Result<ExportReport, ExportError> {
    let _execution_guard = crate::execution::PYTHON_EXECUTION_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Python::with_gil(|py| {
        let namespace = PyDict::new(py);
        crate::measurement::define_measurement(py, &namespace)?;
        py.run(EXPORT_SOURCE, Some(&namespace), None)?;
        let model_path = model.0.display().to_string();
        let file = path.display().to_string();

        if format.is_drawing() {
            let Some(plane) = plane else {
                return Err(ExportError::NoPlane(format.label()));
            };
            let written: bool = namespace
                .get_item("export_drawing")?
                .expect("export source defines export_drawing()")
                .call1((
                    &model_path,
                    format.extension(),
                    &file,
                    (plane.origin, plane.x_axis, plane.normal),
                ))?
                .extract()?;
            return if written {
                Ok(ExportReport::new(format))
            } else {
                Err(ExportError::WriteFailed(file))
            };
        }

        let export_solid = namespace
            .get_item("export_solid")?
            .expect("export source defines export_solid()");
        let attempt = |convert: bool| -> Result<ExportReport, ExportError> {
            let outcome = export_solid.call1((&model_path, format.extension(), &file, convert))?;
            if outcome.is_none() {
                return Err(ExportError::WriteFailed(file.clone()));
            }
            let mut report = extract_report(format, &outcome)?;
            if convert {
                let retained = report.retained.volume;
                let written = report
                    .written
                    .as_ref()
                    .map_or(0.0, |figures| figures.volume);
                report.conversion = Some(Conversion {
                    volume_deviation: (written - retained)
                        / retained.abs().max(GEOMETRIC_CONFUSION),
                });
            }
            Ok(report)
        };

        // Whatever went wrong, nothing that failed the proof stays at the
        // path: a refused file, or the first attempt's file when the
        // converted second attempt did not get as far as a report.
        let outcome = prove(format, attempt);
        if outcome.as_ref().map_or(true, ExportReport::refused) && path.exists() {
            std::fs::remove_file(path).map_err(|error| ExportError::RemoveFailed(file, error))?;
        }
        outcome
    })
}

/// The gate's decision over write attempts: a plain write first, and for a
/// STEP that did not reproduce the part, one more with the faces the writer
/// cannot carry converted. The final attempt's report is the verdict; a
/// mesh gets no second attempt, since nothing about a mesh writer's loss is
/// a conversion away. Kept apart from the Python so the decision is testable
/// without a kernel.
fn prove(
    format: ExportFormat,
    mut attempt: impl FnMut(bool) -> Result<ExportReport, ExportError>,
) -> Result<ExportReport, ExportError> {
    let report = attempt(false)?;
    if report.refused() && format == ExportFormat::Step {
        return attempt(true);
    }
    Ok(report)
}

fn extract_report(format: ExportFormat, outcome: &Bound<'_, PyAny>) -> PyResult<ExportReport> {
    let figures = |item: &Bound<'_, PyAny>| -> PyResult<ShapeFigures> {
        Ok(ShapeFigures {
            solids: item.get_item("solids")?.extract()?,
            shells: item.get_item("shells")?.extract()?,
            faces: item.get_item("faces")?.extract()?,
            volume: item.get_item("volume")?.extract()?,
            size: item.get_item("size")?.extract()?,
        })
    };
    let mut lost_faces = Vec::new();
    for face in outcome.get_item("lost_faces")?.try_iter()? {
        let face = face?;
        lost_faces.push(LostFace {
            surface: face.get_item("surface")?.extract()?,
            curves: face.get_item("curves")?.extract()?,
            area: face.get_item("area")?.extract()?,
        });
    }
    Ok(ExportReport {
        retained: figures(&outcome.get_item("retained")?)?,
        written: Some(figures(&outcome.get_item("written")?)?),
        lost_faces,
        ..ExportReport::new(format)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadmark_core::export::{ExportVerdict, ShapeFigures};

    fn reproduced(format: ExportFormat) -> ExportReport {
        let figures = ShapeFigures {
            solids: 1,
            shells: 1,
            faces: 7,
            volume: 100.0,
            size: [10.0; 3],
            ..ShapeFigures::default()
        };
        ExportReport {
            retained: figures.clone(),
            written: Some(figures),
            ..ExportReport::new(format)
        }
    }

    fn lost_everything(format: ExportFormat) -> ExportReport {
        ExportReport {
            written: Some(ShapeFigures::default()),
            ..reproduced(format)
        }
    }

    /// Replays scripted attempt outcomes and records whether each was asked
    /// to convert.
    fn scripted(
        outcomes: Vec<ExportReport>,
        asked: &mut Vec<bool>,
    ) -> impl FnMut(bool) -> Result<ExportReport, ExportError> {
        let mut outcomes = outcomes.into_iter();
        move |convert| {
            asked.push(convert);
            Ok(outcomes
                .next()
                .expect("an attempt the script did not expect"))
        }
    }

    #[test]
    fn a_step_that_reproduces_the_part_on_the_plain_write_is_not_converted() {
        let mut asked = Vec::new();
        let report = prove(
            ExportFormat::Step,
            scripted(vec![reproduced(ExportFormat::Step)], &mut asked),
        )
        .unwrap();
        assert_eq!(report.verdict(), ExportVerdict::Reproduced);
        assert_eq!(asked, [false]);
    }

    #[test]
    fn a_step_that_lost_faces_is_written_again_converted_and_the_second_report_stands() {
        let mut asked = Vec::new();
        let converted = ExportReport {
            conversion: Some(Conversion {
                volume_deviation: 0.001,
            }),
            ..reproduced(ExportFormat::Step)
        };
        let report = prove(
            ExportFormat::Step,
            scripted(
                vec![lost_everything(ExportFormat::Step), converted],
                &mut asked,
            ),
        )
        .unwrap();
        assert_eq!(asked, [false, true]);
        assert_eq!(report.verdict(), ExportVerdict::Reproduced);
        assert!(report.conversion.is_some());
    }

    #[test]
    fn a_step_still_lost_after_conversion_is_refused() {
        let mut asked = Vec::new();
        let report = prove(
            ExportFormat::Step,
            scripted(
                vec![
                    lost_everything(ExportFormat::Step),
                    lost_everything(ExportFormat::Step),
                ],
                &mut asked,
            ),
        )
        .unwrap();
        assert_eq!(asked, [false, true]);
        assert_eq!(report.verdict(), ExportVerdict::Refused);
    }

    #[test]
    fn a_mesh_that_did_not_reproduce_the_part_gets_no_conversion_attempt() {
        let mut asked = Vec::new();
        let report = prove(
            ExportFormat::Stl,
            scripted(vec![lost_everything(ExportFormat::Stl)], &mut asked),
        )
        .unwrap();
        assert_eq!(asked, [false]);
        assert_eq!(report.verdict(), ExportVerdict::Refused);
    }
}
