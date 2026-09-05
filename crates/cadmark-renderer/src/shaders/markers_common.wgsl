// Screen-space sizing shared by every pass that draws or picks an edge or
// a vertex marker. Both the visible pass and the picking pass expand their
// geometry through these functions with separate visible and hit extents.

// Mirror of the Rust `MarkerExtent` in `markers.rs`: marker half-extents
// in clip space, per axis.
struct MarkerExtent {
    edge_half_width_ndc: vec2<f32>,
    vertex_radius_ndc: vec2<f32>,
}

// Offset one corner of an edge segment's quad. `side` (-1 or +1) picks the
// side of the line; `cap` (-1 or +1) extends the end by a half width so
// consecutive segments meet without a gap; `end_sign` says which endpoint
// the corner sits at. Dividing the screen-space
// direction by the half-extent makes the units isotropic, so the quad is
// the same number of pixels wide on both axes.
fn expand_edge(
    clip_here: vec4<f32>,
    clip_other: vec4<f32>,
    side: f32,
    cap: f32,
    end_sign: f32,
    extent: MarkerExtent,
) -> vec4<f32> {
    let k = max(extent.edge_half_width_ndc, vec2<f32>(1e-9, 1e-9));
    let here = clip_here.xy / max(abs(clip_here.w), 1e-6);
    let other = clip_other.xy / max(abs(clip_other.w), 1e-6);

    var tangent = (other - here) / k;
    let length_units = length(tangent);
    if length_units < 1e-6 {
        tangent = vec2<f32>(1.0, 0.0);
    } else {
        tangent = tangent / length_units;
    }
    // The tangent points inwards from whichever end this corner sits at,
    // so the normal is turned back to the segment's own orientation. Left
    // as it is, the two ends pick opposite sides and the quad folds into
    // a bowtie that covers half the width it should.
    let normal = vec2<f32>(-tangent.y, tangent.x) * end_sign;

    let offset = (normal * side + tangent * cap) * k;
    // Clip space is divided by w downstream, so scale the offset by w to
    // land the corner where the screen-space offset asks for it.
    return vec4<f32>(
        clip_here.xy + offset * clip_here.w,
        clip_here.z,
        clip_here.w,
    );
}

// Offset one corner of a vertex marker's quad. `corner` is one of the four
// combinations of -1 and +1.
fn expand_marker(clip: vec4<f32>, corner: vec2<f32>, extent: MarkerExtent) -> vec4<f32> {
    let offset = corner * extent.vertex_radius_ndc;
    return vec4<f32>(clip.xy + offset * clip.w, clip.z, clip.w);
}

// Hard boundary of the picking disc. Visible discs use smooth coverage.
fn marker_covers(corner: vec2<f32>) -> bool {
    return length(corner) <= 1.0;
}
