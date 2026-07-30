import importlib as _cadmark_importlib
import importlib.metadata as _cadmark_metadata
import inspect as _cadmark_inspect
import sys as _cadmark_sys

import OCP as _cadmark_ocp
import build123d as _cadmark_build123d
from OCP.BRepAlgoAPI import (
    BRepAlgoAPI_Common as _cadmark_expected_common,
    BRepAlgoAPI_Cut as _cadmark_expected_cut,
    BRepAlgoAPI_Fuse as _cadmark_expected_fuse,
)
from OCP.BRepBuilderAPI import BRepBuilderAPI_Copy as _cadmark_expected_copy
from OCP.BRepFilletAPI import (
    BRepFilletAPI_MakeChamfer as _cadmark_expected_chamfer,
    BRepFilletAPI_MakeFillet as _cadmark_expected_fillet,
)
from OCP.BRepPrimAPI import (
    BRepPrimAPI_MakeBox as _cadmark_expected_box,
    BRepPrimAPI_MakeCylinder as _cadmark_expected_cylinder,
)
from OCP.ShapeUpgrade import (
    ShapeUpgrade_UnifySameDomain as _cadmark_expected_cleanup,
)
from OCP.TopAbs import (
    TopAbs_EDGE as _cadmark_edge,
    TopAbs_FACE as _cadmark_face,
    TopAbs_VERTEX as _cadmark_vertex,
)
from OCP.TopExp import TopExp as _cadmark_top_exp
from OCP.TopTools import TopTools_IndexedMapOfShape as _cadmark_indexed_map
from OCP.TopoDS import TopoDS_Shape as _cadmark_shape_type


class UnsupportedRuntimeVersion(RuntimeError):
    pass


class ProvenanceWrapperError(RuntimeError):
    pass


class UnsupportedProvenanceCapture(RuntimeError):
    pass


_cadmark_manifest = (
    ("build123d.topology.three_d", "BRepPrimAPI_MakeBox", _cadmark_expected_box, "Box", "primitive"),
    ("build123d.topology.three_d", "BRepPrimAPI_MakeCylinder", _cadmark_expected_cylinder, "Cylinder", "primitive"),
    ("build123d.topology.shape_core", "BRepAlgoAPI_Fuse", _cadmark_expected_fuse, "BooleanFuse", "history"),
    ("build123d.topology.shape_core", "BRepAlgoAPI_Cut", _cadmark_expected_cut, "BooleanCut", "history"),
    ("build123d.topology.three_d", "BRepAlgoAPI_Common", _cadmark_expected_common, "BooleanCommon", "history"),
    ("build123d.topology.three_d", "BRepFilletAPI_MakeFillet", _cadmark_expected_fillet, "Fillet", "history"),
    ("build123d.topology.three_d", "BRepFilletAPI_MakeChamfer", _cadmark_expected_chamfer, "Chamfer", "history"),
    ("build123d.topology.shape_core", "BRepBuilderAPI_Copy", _cadmark_expected_copy, None, "copy"),
    ("build123d.topology.shape_core", "ShapeUpgrade_UnifySameDomain", _cadmark_expected_cleanup, None, "cleanup"),
)


def _cadmark_unique_candidates(candidates):
    seen = set()
    result = []
    for candidate in candidates:
        key = (candidate["operation_id"], candidate["relation"])
        if key not in seen:
            seen.add(key)
            result.append(candidate)
    result.sort(key=lambda item: (item["operation_id"], item["relation"]))
    return result


class _CadmarkSession:
    def __init__(self, filename):
        self.filename = filename
        self.operations = []
        self.tombstones = []
        self.registry = []
        self.originals = []
        self.final_maps = {}
        self._next_operation_id = 1

    def user_line(self):
        frame = _cadmark_inspect.currentframe()
        try:
            frame = frame.f_back
            while frame is not None:
                if frame.f_code.co_filename == self.filename:
                    return frame.f_lineno
                frame = frame.f_back
        finally:
            del frame
        raise UnsupportedProvenanceCapture(
            f"no user frame for provenance capture in {self.filename}"
        )

    @staticmethod
    def unwrap(value):
        if hasattr(value, "wrapped"):
            value = value.wrapped
        return value

    def collect_shapes(self, value):
        found = []

        def visit(item):
            item = self.unwrap(item)
            if isinstance(item, _cadmark_shape_type):
                found.append(item)
                return
            if isinstance(item, dict):
                for nested in item.values():
                    visit(nested)
                return
            if isinstance(item, (str, bytes)):
                return
            if isinstance(item, (tuple, list, set)):
                for nested in item:
                    visit(nested)
                return
            try:
                for nested in item:
                    visit(nested)
            except (TypeError, RuntimeError):
                return

        visit(value)
        unique = []
        for shape in found:
            if not any(shape.IsSame(existing) for existing in unique):
                unique.append(shape)
        return unique

    @staticmethod
    def make_map(shape, topology_kind):
        result = _cadmark_indexed_map()
        _cadmark_top_exp.MapShapes_s(shape, topology_kind, result)
        return result

    @staticmethod
    def map_values(indexed_map):
        return [indexed_map.FindKey(index) for index in range(1, indexed_map.Extent() + 1)]

    def topology(self, shape):
        return (
            ("face", self.map_values(self.make_map(shape, _cadmark_face))),
            ("edge", self.map_values(self.make_map(shape, _cadmark_edge))),
            ("vertex", self.map_values(self.make_map(shape, _cadmark_vertex))),
        )

    def lookup(self, shape, partner_fallback=False):
        candidates = []
        for record in self.registry:
            if shape.IsSame(record["shape"]):
                candidates.extend(record["candidates"])
        if not candidates and partner_fallback:
            for record in self.registry:
                if record["allow_partner"] and shape.IsPartner(record["shape"]):
                    candidates.extend(record["candidates"])
        return _cadmark_unique_candidates(candidates)

    def register(self, shape, candidates, allow_partner=False, replace=False):
        candidates = _cadmark_unique_candidates(candidates)
        if candidates:
            if replace:
                self.registry = [
                    record
                    for record in self.registry
                    if not shape.IsSame(record["shape"])
                ]
            self.registry.append(
                {
                    "shape": shape,
                    "candidates": candidates,
                    "allow_partner": allow_partner,
                }
            )

    def new_operation(self, source_line, operation, api_class):
        operation_id = self._next_operation_id
        self._next_operation_id += 1
        self.operations.append(
            {
                "operation_id": operation_id,
                "source_line": source_line,
                "operation": operation,
                "api_class": api_class,
            }
        )
        return operation_id

    @staticmethod
    def history_results(builder, method, shape):
        try:
            result = getattr(builder, method)(shape)
            return list(result)
        except (AttributeError, TypeError, RuntimeError):
            try:
                history = builder.History()
                result = getattr(history, method)(shape)
                return list(result)
            except (AttributeError, TypeError, RuntimeError):
                return []

    @staticmethod
    def contains(result, child, kind):
        indexed = _CadmarkSession.make_map(
            result,
            {"face": _cadmark_face, "edge": _cadmark_edge, "vertex": _cadmark_vertex}[kind],
        )
        return indexed.FindIndex(child) > 0

    def capture_primitive(self, builder, result, operation, api_class):
        operation_id = self.new_operation(
            builder._cadmark_source_line, operation, api_class
        )
        candidate = {"operation_id": operation_id, "relation": "Generated"}
        for _kind, shapes in self.topology(result):
            for shape in shapes:
                self.register(
                    shape, [candidate], allow_partner=True, replace=True
                )

    def capture_history(self, builder, result, operation, api_class):
        operation_id = self.new_operation(
            builder._cadmark_source_line, operation, api_class
        )
        inputs_by_kind = {"face": [], "edge": [], "vertex": []}
        for root in builder._cadmark_inputs:
            for input_kind, shapes in self.topology(root):
                for shape in shapes:
                    if not any(
                        shape.IsSame(existing)
                        for existing in inputs_by_kind[input_kind]
                    ):
                        inputs_by_kind[input_kind].append(shape)

        for kind, inputs in inputs_by_kind.items():
            for ordinal, input_shape in enumerate(inputs):
                try:
                    deleted = bool(builder.IsDeleted(input_shape))
                except (AttributeError, TypeError, RuntimeError):
                    deleted = False
                if deleted:
                    self.tombstones.append(
                        {
                            "operation_id": operation_id,
                            "kind": kind,
                            "ordinal": ordinal,
                        }
                    )

        all_inputs = [
            shape for inputs in inputs_by_kind.values() for shape in inputs
        ]
        for kind, outputs in self.topology(result):
            for output in outputs:
                direct = []
                descendants = []
                for input_shape in inputs_by_kind[kind]:
                    for relation, method in (
                        ("Generated", "Generated"),
                        ("Modified", "Modified"),
                    ):
                        for changed in self.history_results(builder, method, input_shape):
                            if output.IsSame(changed):
                                direct.append(
                                    {
                                        "operation_id": operation_id,
                                        "relation": relation,
                                    }
                                )
                            elif self.contains(changed, output, kind):
                                descendants.append(
                                    {
                                        "operation_id": operation_id,
                                        "relation": relation + "Descendant",
                                    }
                                )
                if not direct and not descendants:
                    for input_shape in all_inputs:
                        for relation, method in (
                            ("Generated", "Generated"),
                            ("Modified", "Modified"),
                        ):
                            for changed in self.history_results(
                                builder, method, input_shape
                            ):
                                if self.contains(changed, output, kind):
                                    descendants.append(
                                        {
                                            "operation_id": operation_id,
                                            "relation": relation + "Descendant",
                                        }
                                    )
                current = direct or descendants
                if current:
                    relation_priority = (
                        "Generated",
                        "Modified",
                        "GeneratedDescendant",
                        "ModifiedDescendant",
                    )
                    selected = next(
                        relation
                        for relation in relation_priority
                        if any(
                            candidate["relation"] == relation
                            for candidate in current
                        )
                    )
                    candidates = [
                        {
                            "operation_id": operation_id,
                            "relation": selected,
                        }
                    ]
                    self.register(output, candidates, replace=True)
                else:
                    self.register(output, self.lookup(output))

    def capture_transport(self, builder, result, allow_partner):
        inputs_by_kind = {"face": [], "edge": [], "vertex": []}
        for root in builder._cadmark_inputs:
            for input_kind, shapes in self.topology(root):
                for shape in shapes:
                    if not any(
                        shape.IsSame(existing)
                        for existing in inputs_by_kind[input_kind]
                    ):
                        inputs_by_kind[input_kind].append(shape)
        for kind, outputs in self.topology(result):
            inputs = inputs_by_kind[kind]
            for output in outputs:
                candidates = []
                for input_shape in inputs:
                    changed_shapes = self.history_results(
                        builder, "Modified", input_shape
                    ) + self.history_results(builder, "Generated", input_shape)
                    if output.IsSame(input_shape) or any(
                        output.IsSame(changed) or self.contains(changed, output, kind)
                        for changed in changed_shapes
                    ):
                        candidates.extend(self.lookup(input_shape))
                if not candidates:
                    candidates.extend(self.lookup(output))
                self.register(output, candidates, allow_partner=allow_partner)

    def capture(self, builder, result, operation, adapter, api_class):
        result = self.unwrap(result)
        if result.IsNull():
            return
        if adapter == "primitive":
            self.capture_primitive(builder, result, operation, api_class)
        elif adapter == "history":
            self.capture_history(builder, result, operation, api_class)
        elif adapter == "copy":
            self.capture_transport(builder, result, allow_partner=True)
        elif adapter == "cleanup":
            self.capture_transport(builder, result, allow_partner=False)

    def wrap(self, original, operation, adapter, api_class):
        session = self

        def init(instance, *args, **kwargs):
            instance._cadmark_source_line = session.user_line()
            instance._cadmark_inputs = session.collect_shapes((args, kwargs))
            instance._cadmark_captured = False
            original.__init__(instance, *args, **kwargs)

        def shape(instance, *args, **kwargs):
            result = original.Shape(instance, *args, **kwargs)
            if not instance._cadmark_captured:
                instance._cadmark_captured = True
                session.capture(instance, result, operation, adapter, api_class)
            return result

        namespace = {"__init__": init, "Shape": shape, "__module__": original.__module__}

        for method_name in ("SetArguments", "SetTools", "Add", "Perform", "Initialize"):
            if not hasattr(original, method_name):
                continue
            original_method = getattr(original, method_name)

            def capture_input(instance, *args, _method=original_method, **kwargs):
                for captured in session.collect_shapes((args, kwargs)):
                    if not any(
                        captured.IsSame(existing)
                        for existing in instance._cadmark_inputs
                    ):
                        instance._cadmark_inputs.append(captured)
                return _method(instance, *args, **kwargs)

            namespace[method_name] = capture_input

        return type(api_class, (original,), namespace)

    def install(self, manifest=None):
        manifest = _cadmark_manifest if manifest is None else manifest
        installed = []
        try:
            for module_name, attribute, expected, operation, adapter in manifest:
                module = _cadmark_importlib.import_module(module_name)
                original = getattr(module, attribute)
                if not isinstance(original, type) or not issubclass(original, expected):
                    raise ProvenanceWrapperError(
                        f"{module_name}.{attribute} is not the expected OCP base"
                    )
                wrapped = self.wrap(original, operation, adapter, attribute)
                setattr(module, attribute, wrapped)
                if getattr(module, attribute) is not wrapped:
                    raise ProvenanceWrapperError(
                        f"failed to replace {module_name}.{attribute}"
                    )
                installed.append((module, attribute, original))
            self.originals = installed
        except Exception:
            for module, attribute, original in reversed(installed):
                setattr(module, attribute, original)
            raise

    def restore(self):
        failures = []
        for module, attribute, original in reversed(self.originals):
            try:
                setattr(module, attribute, original)
                if getattr(module, attribute) is not original:
                    failures.append(f"{module.__name__}.{attribute}")
            except Exception:
                failures.append(f"{module.__name__}.{attribute}")
        self.originals = []
        if failures:
            raise ProvenanceWrapperError(
                "failed to restore " + ", ".join(failures)
            )

    def finalise(self, shape):
        shape = self.unwrap(shape)
        self.final_maps = {
            "face": self.make_map(shape, _cadmark_face),
            "edge": self.make_map(shape, _cadmark_edge),
            "vertex": self.make_map(shape, _cadmark_vertex),
        }
        result = {
            "schema_version": 1,
            "operations": list(self.operations),
            "faces": [],
            "edges": [],
            "vertices": [],
            "tombstones": list(self.tombstones),
        }
        for kind, output_name in (
            ("face", "faces"),
            ("edge", "edges"),
            ("vertex", "vertices"),
        ):
            for final_shape in self.map_values(self.final_maps[kind]):
                result[output_name].append(
                    {
                        "candidates": self.lookup(
                            final_shape, partner_fallback=True
                        )
                    }
                )
        return result

    def topology_index(self, shape, kind):
        indexed = self.final_maps[kind]
        index = indexed.FindIndex(shape)
        if index == 0:
            raise UnsupportedProvenanceCapture(
                f"{kind} is absent from the final topology map"
            )
        return index - 1


def _cadmark_validate_runtime(actual=None):
    if actual is None:
        actual = (
            _cadmark_sys.version_info[:2],
            _cadmark_build123d.__version__,
            _cadmark_ocp.__version__,
            _cadmark_metadata.version("cadquery-ocp-novtk"),
        )
    expected = ((3, 12), "0.11.1", "7.9.3.1", "7.9.3.1.1")
    if actual != expected:
        raise UnsupportedRuntimeVersion(
            f"expected Python/build123d/OCP/distribution {expected}, got {actual}"
        )


def _cadmark_install(filename, manifest=None, actual_runtime=None):
    _cadmark_validate_runtime(actual_runtime)
    session = _CadmarkSession(filename)
    session.install(manifest)
    return session
