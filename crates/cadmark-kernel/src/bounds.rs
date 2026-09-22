// Exact bounding box of a shape, computed from its surfaces rather than
// a triangulation, at a fraction of OCCT's whole-shape cost.
//
// `BRepBndLib::AddOptimal` on a whole shape runs a numerical extremum
// search over every face and edge. Most of those searches cannot move
// the box: an element whose cheap tolerance-padded box already lies
// inside the exact box found so far contributes nothing. Seeding the
// box with the vertices and visiting elements largest-first leaves only
// the elements that can extend it, which on a real model is a few per
// cent of the total.

use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Defines `_cadmark_exact_bounds(shape) -> Bnd_Box` in a namespace.
pub(crate) const BOUNDS_SOURCE: &std::ffi::CStr = c"
from OCP.Bnd import Bnd_Box as _cadmark_bnd_box
from OCP.BRep import BRep_Tool as _cadmark_brep_tool
from OCP.BRepBndLib import BRepBndLib as _cadmark_bnd_lib
from OCP.TopAbs import (
    TopAbs_EDGE as _cadmark_bounds_edge,
    TopAbs_FACE as _cadmark_bounds_face,
    TopAbs_VERTEX as _cadmark_bounds_vertex,
)
from OCP.TopExp import TopExp_Explorer as _cadmark_bounds_explorer
from OCP.TopoDS import TopoDS as _cadmark_bounds_topods


def _cadmark_bounds_members(shape, kind):
    explorer = _cadmark_bounds_explorer(shape, kind)
    while explorer.More():
        yield explorer.Current()
        explorer.Next()


def _cadmark_exact_bounds(shape):
    \"\"\"The optimal (surface-exact) bounding box of a shape.

    Identical to BRepBndLib.AddOptimal_s over the whole shape without
    triangulation or tolerance, but the extremum search runs only on the
    elements whose loose box reaches outside the exact box so far.
    \"\"\"
    exact = _cadmark_bnd_box()
    for vertex in _cadmark_bounds_members(shape, _cadmark_bounds_vertex):
        exact.Add(_cadmark_brep_tool.Pnt_s(_cadmark_bounds_topods.Vertex_s(vertex)))
    candidates = []
    for kind in (_cadmark_bounds_face, _cadmark_bounds_edge):
        for member in _cadmark_bounds_members(shape, kind):
            loose = _cadmark_bnd_box()
            _cadmark_bnd_lib.Add_s(member, loose, False)
            if not loose.IsVoid():
                candidates.append((loose.SquareExtent(), member, loose))
    candidates.sort(key=lambda candidate: -candidate[0])
    for _extent, member, loose in candidates:
        if not exact.IsVoid():
            x_min, y_min, z_min, x_max, y_max, z_max = exact.Get()
            l_x_min, l_y_min, l_z_min, l_x_max, l_y_max, l_z_max = loose.Get()
            if (
                l_x_min >= x_min and l_y_min >= y_min and l_z_min >= z_min
                and l_x_max <= x_max and l_y_max <= y_max and l_z_max <= z_max
            ):
                continue
        _cadmark_bnd_lib.AddOptimal_s(member, exact, False, False)
    return exact
";

/// Make `_cadmark_exact_bounds` available in `namespace`.
pub(crate) fn define_exact_bounds(py: Python<'_>, namespace: &Bound<'_, PyDict>) -> PyResult<()> {
    py.run(BOUNDS_SOURCE, Some(namespace), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selective search returns the same box as OCCT's whole-shape
    /// optimal search on curved, concave and multi-solid shapes.
    #[test]
    fn exact_bounds_match_whole_shape_optimal_search() {
        crate::execution::discover_and_activate_venv().unwrap();
        let _guard = crate::execution::PYTHON_EXECUTION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::with_gil(|py| {
            let namespace = PyDict::new(py);
            define_exact_bounds(py, &namespace).unwrap();
            py.run(
                c"
from build123d import *
from OCP.Bnd import Bnd_Box
from OCP.BRepBndLib import BRepBndLib

_cadmark_test_shapes = [
    Sphere(10).wrapped,
    Torus(20, 5).rotate(Axis.X, 30).wrapped,
    (Box(10, 10, 10) - Cylinder(3, 20)).wrapped,
    (Box(10, 10, 10) + Box(4, 4, 4).moved(Location((7, 7, 7)))).wrapped,
    Cylinder(5, 30).rotate(Axis.Y, 37).wrapped,
]
_cadmark_test_results = []
for _cadmark_test_shape in _cadmark_test_shapes:
    reference = Bnd_Box()
    BRepBndLib.AddOptimal_s(_cadmark_test_shape, reference, False, False)
    exact = _cadmark_exact_bounds(_cadmark_test_shape)
    _cadmark_test_results.append(
        max(abs(a - b) for a, b in zip(exact.Get(), reference.Get()))
    )
",
                Some(&namespace),
                None,
            )
            .unwrap();
            let differences: Vec<f64> = namespace
                .get_item("_cadmark_test_results")
                .unwrap()
                .unwrap()
                .extract()
                .unwrap();
            assert_eq!(differences.len(), 5);
            for difference in differences {
                assert!(difference < 1e-6, "{difference}");
            }
        });
    }
}
