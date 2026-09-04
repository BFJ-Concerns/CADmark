// The 3D viewport's GPU side: the resources egui_wgpu keeps between
// frames, the paint callback that draws the scene and services pick and
// hover readbacks, and the small state machine that decides what a
// completed readback means. Everything else about the viewport — input,
// the overlay, the gizmo — is in `app.rs`; everything a pass draws is in
// the renderer's `viewport` module.

use std::sync::{Arc, Mutex};

use cadmark_core::geometry::{PartId, TopologyElement};
use cadmark_kernel::protocol::ExecutedPart;
use cadmark_renderer::mesh::GpuMesh;
use cadmark_renderer::picking::PickingPass;
use cadmark_renderer::pipeline::{
    MeshUniforms, RenderPipelines, SimpleUniforms, ViewportMarker, upload_mesh,
};
use cadmark_renderer::viewport::{
    copy_pick_pixel, decode_pick_result, render_part_picking, render_picking, render_scene,
};

/// GPU resources for the 3D viewport, stored in egui_wgpu's
/// `callback_resources` so both `prepare()` and `paint()` can reach them.
pub struct ViewportResources {
    pipelines: RenderPipelines,
    picking: PickingPass,
    meshes: Vec<GpuMesh>,
    /// Part whose local face and edge IDs topology picking reads.
    active_part: Option<usize>,
    /// Pick attempt submitted through the independent readback encoder. Its
    /// marker proves whether those commands completed before bytes are trusted.
    pick_attempt: Option<PickAttempt>,
    submission_marker_source: wgpu::Buffer,
    submission_marker_staging: wgpu::Buffer,
    next_submission_token: u32,
    retry_pick: Option<((u32, u32), bool)>,
    /// Decoded pick result from the most recent readback, waiting
    /// for `update()` to consume it.
    pick_result: Option<CompletedPick>,
    /// Per-frame hover picking. Unlike a click, a hover readback is never
    /// waited for: the mapped result is collected on a later frame, and
    /// a frame whose readback is still in flight issues no new one.
    hover: HoverPick,
    /// Last-known viewport size in physical pixels — triggers resize
    /// of the picking texture and depth buffer when it changes.
    viewport_size: (u32, u32),
}

impl ViewportResources {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        // Start with a reasonable default size — the first frame's
        // prepare() will resize to the actual viewport.
        let (w, h) = (800, 600);
        Self {
            pipelines: RenderPipelines::new(device, format, w, h),
            picking: PickingPass::new(device, w, h),
            meshes: Vec::new(),
            active_part: None,
            pick_attempt: None,
            submission_marker_source: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pick_submission_marker_source"),
                size: 4,
                usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            submission_marker_staging: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pick_submission_marker_staging"),
                size: 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            next_submission_token: 1,
            retry_pick: None,
            pick_result: None,
            hover: HoverPick::new(device),
            viewport_size: (w, h),
        }
    }

    /// Replace every independently rendered part on the GPU.
    pub fn set_parts(&mut self, device: &wgpu::Device, parts: &[ExecutedPart]) {
        self.meshes = parts
            .iter()
            .map(|part| upload_mesh(device, &part.mesh, PartId(part.id)))
            .collect();
        self.active_part = parts.len().checked_sub(1);
    }

    pub fn set_active_part(&mut self, id: u32) {
        self.active_part = self.meshes.get(id as usize).map(|_| id as usize);
    }

    /// Forget every pending pick: the model they were for is gone.
    pub fn clear_picks(&mut self) {
        self.pick_result = None;
        self.pick_attempt = None;
        self.retry_pick = None;
        self.hover.result = None;
    }

    /// Take the results the last frame's readbacks produced, and whether
    /// a hover readback is still in flight.
    pub fn take_results(&mut self) -> (Option<CompletedPick>, Option<u32>, bool) {
        (
            self.pick_result.take(),
            self.hover.result.take(),
            self.hover.in_flight.is_some(),
        )
    }
}

pub enum CompletedPick {
    Hit(TopologyElement),
    Background,
    ReadbackFailed,
}

/// Where a buffer-mapping callback leaves its outcome for a later frame.
type MapStatus = Arc<Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>;

/// Asynchronous readback of the element under the cursor.
struct HoverPick {
    /// One-pixel staging buffer, separate from the click path's so the two
    /// readbacks never contend for a mapping.
    staging: wgpu::Buffer,
    /// Mapping in progress; the callback stores its status here and a later
    /// frame's poll reads it.
    in_flight: Option<MapStatus>,
    /// Picking ID under the cursor from the most recent completed readback
    /// (0 = background), waiting for `update()` to consume it.
    result: Option<u32>,
}

impl HoverPick {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            staging: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hover_pick_staging"),
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            in_flight: None,
            result: None,
        }
    }

    /// Collect a finished readback without blocking. A mapping that has not
    /// completed stays in flight; a failed one is dropped, and the next
    /// frame simply picks again.
    fn collect(&mut self, device: &wgpu::Device) {
        let Some(status) = &self.in_flight else {
            return;
        };
        let _ = device.poll(wgpu::Maintain::Poll);
        let status = status.lock().map(|mut slot| slot.take()).unwrap_or(None);
        match status {
            Some(Ok(())) => {
                let data = self.staging.slice(..4).get_mapped_range();
                let pixel = [data[0], data[1], data[2], data[3]];
                drop(data);
                self.staging.unmap();
                self.result = Some(cadmark_renderer::picking::colour_to_id(pixel));
                self.in_flight = None;
            }
            Some(Err(error)) => {
                log::debug!("Hover readback failed: {error}");
                self.in_flight = None;
            }
            None => {}
        }
    }

    /// Begin mapping the staging buffer once the copy into it is submitted.
    fn begin_readback(&mut self) {
        let slot = Arc::new(Mutex::new(None));
        let writer = Arc::clone(&slot);
        self.staging
            .slice(..4)
            .map_async(wgpu::MapMode::Read, move |status| {
                if let Ok(mut slot) = writer.lock() {
                    *slot = Some(status);
                }
            });
        self.in_flight = Some(slot);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PickAttempt {
    pixel: (u32, u32),
    submission_token: u32,
    part_pick: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum PickSubmissionDecision {
    ReadPick,
    Retry((u32, u32), bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickPass {
    Topology,
    Part,
}

fn pick_submission_decision(attempt: PickAttempt, marker_bytes: [u8; 4]) -> PickSubmissionDecision {
    if u32::from_le_bytes(marker_bytes) == attempt.submission_token {
        PickSubmissionDecision::ReadPick
    } else {
        PickSubmissionDecision::Retry(attempt.pixel, attempt.part_pick)
    }
}

fn next_pick_request(
    part_pick_request: Option<(u32, u32)>,
    topology_pick_request: Option<(u32, u32)>,
    retry_pick: Option<((u32, u32), bool)>,
) -> Option<((u32, u32), PickPass)> {
    part_pick_request
        .map(|pixel| (pixel, PickPass::Part))
        .or(topology_pick_request.map(|pixel| (pixel, PickPass::Topology)))
        .or_else(|| {
            retry_pick.map(|(pixel, part_pick)| {
                (
                    pixel,
                    if part_pick {
                        PickPass::Part
                    } else {
                        PickPass::Topology
                    },
                )
            })
        })
}

/// What a completed click readback means for the selection.
#[derive(Debug, PartialEq)]
pub enum PickTransition {
    Waiting,
    Hit(TopologyElement, (f32, f32)),
    Background,
    ReadbackFailed,
}

/// Resolve a completed readback against the screen anchor of the click
/// that requested it. The anchor is kept while the readback is incomplete
/// and consumed when it lands.
pub fn completed_pick_transition(
    completed: Option<CompletedPick>,
    pick_in_flight: &mut Option<(f32, f32)>,
) -> PickTransition {
    match completed {
        None => PickTransition::Waiting,
        Some(CompletedPick::Hit(element)) => match pick_in_flight.take() {
            Some(screen_pos) => PickTransition::Hit(element, screen_pos),
            None => PickTransition::Background,
        },
        Some(CompletedPick::Background) => {
            pick_in_flight.take();
            PickTransition::Background
        }
        Some(CompletedPick::ReadbackFailed) => {
            pick_in_flight.take();
            PickTransition::ReadbackFailed
        }
    }
}

/// Per-frame data passed into the egui_wgpu paint callback.
/// Carries everything that changes frame-to-frame (uniforms,
/// whether a pick was requested, viewport dimensions).
pub struct ViewportCallback {
    pub mesh_uniforms: MeshUniforms,
    /// Picking IDs of the candidate-footprint highlight, uploaded whole:
    /// the buffer grows to fit rather than the set being trimmed to fit.
    pub highlight_ids: Vec<u32>,
    pub simple_uniforms: SimpleUniforms,
    /// Application-owned marker data; the renderer sees only topology IDs and colours.
    pub markers: Vec<ViewportMarker>,
    /// Pixel coordinates within the viewport to read back for
    /// picking, if the user clicked this frame.
    pub pick_request: Option<(u32, u32)>,
    /// A whole-part click uses its own picking pass and ID range.
    pub part_pick_request: Option<(u32, u32)>,
    /// Pixel coordinates under the cursor, if it is over the viewport.
    pub hover_request: Option<(u32, u32)>,
    /// Viewport size in physical pixels (for resize detection).
    pub viewport_size: (u32, u32),
    /// Background colour in the offscreen target's own colour space.
    pub clear_colour: wgpu::Color,
}

impl eframe::egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &eframe::egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut eframe::egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = callback_resources.get_mut::<ViewportResources>() else {
            return Vec::new();
        };

        // Confirm that the independent picking encoder completed before
        // trusting readback bytes. A stale marker retries the exact pixel.
        if let Some(attempt) = res.pick_attempt.take() {
            let slice = res.submission_marker_staging.slice(..4);
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            slice.map_async(wgpu::MapMode::Read, move |status| {
                let _ = tx.send(status);
            });
            device.poll(wgpu::Maintain::Wait);
            match rx.recv() {
                Ok(Ok(())) => {
                    let data = slice.get_mapped_range();
                    let marker_bytes = [data[0], data[1], data[2], data[3]];
                    drop(data);
                    res.submission_marker_staging.unmap();

                    match pick_submission_decision(attempt, marker_bytes) {
                        PickSubmissionDecision::ReadPick => {
                            let slice = res.picking.staging_buffer.slice(..256);
                            let (tx, rx) = std::sync::mpsc::sync_channel(1);
                            slice.map_async(wgpu::MapMode::Read, move |status| {
                                let _ = tx.send(status);
                            });
                            device.poll(wgpu::Maintain::Wait);
                            match rx.recv() {
                                Ok(Ok(())) => {
                                    let data = slice.get_mapped_range();
                                    res.pick_result = Some(match decode_pick_result(&data) {
                                        Some(element) => CompletedPick::Hit(element),
                                        None => CompletedPick::Background,
                                    });
                                    drop(data);
                                    res.picking.staging_buffer.unmap();
                                }
                                _ => {
                                    res.pick_result = Some(CompletedPick::ReadbackFailed);
                                }
                            }
                        }
                        PickSubmissionDecision::Retry(pixel, part_pick) => {
                            log::debug!(
                                "Pick submission was dropped; retrying at ({}, {})",
                                pixel.0,
                                pixel.1
                            );
                            res.retry_pick = Some((pixel, part_pick));
                        }
                    }
                }
                _ => {
                    res.pick_result = Some(CompletedPick::ReadbackFailed);
                }
            }
        }

        // ── Resize picking texture + depth if viewport changed ──
        let (w, h) = self.viewport_size;
        if w > 0 && h > 0 && (w, h) != res.viewport_size {
            res.picking.resize(device, w, h);
            res.pipelines.resize(device, w, h);
            res.viewport_size = (w, h);
        }

        res.hover.collect(device);

        // ── Write per-frame uniforms ──
        res.pipelines
            .reserve_highlights(device, self.highlight_ids.len());
        if !self.highlight_ids.is_empty() {
            queue.write_buffer(
                &res.pipelines.highlight_buffer,
                0,
                bytemuck::cast_slice(&self.highlight_ids),
            );
        }
        queue.write_buffer(
            &res.pipelines.mesh_uniform_buffer,
            0,
            bytemuck::bytes_of(&self.mesh_uniforms),
        );
        res.pipelines.set_markers(device, queue, &self.markers);
        queue.write_buffer(
            &res.pipelines.picking_uniform_buffer,
            0,
            bytemuck::bytes_of(&self.simple_uniforms),
        );

        // ── Pick and hover readbacks ──
        if !res.meshes.is_empty() {
            let topology_meshes = res
                .active_part
                .and_then(|id| res.meshes.get(id))
                .map(std::slice::from_ref)
                .unwrap_or(&res.meshes);
            if self.pick_request.is_some() || self.part_pick_request.is_some() {
                res.retry_pick = None;
            }
            let requested = next_pick_request(
                self.part_pick_request,
                self.pick_request,
                res.retry_pick.take(),
            );
            if let Some(((x, y), pass)) = requested {
                // Submit picking independently. eframe acquires the surface
                // after prepare(); an Outdated surface returns early and drops
                // its shared encoder, but must not drop user selection work.
                let mut pick_encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("pick_readback_encoder"),
                    });
                if pass == PickPass::Part {
                    render_part_picking(
                        &mut pick_encoder,
                        &res.pipelines,
                        &res.picking,
                        &res.meshes,
                    );
                } else {
                    render_picking(
                        &mut pick_encoder,
                        &res.pipelines,
                        &res.picking,
                        &res.meshes,
                        topology_meshes,
                    );
                }

                let submission_token = res.next_submission_token;
                res.next_submission_token = res.next_submission_token.wrapping_add(1).max(1);
                queue.write_buffer(
                    &res.submission_marker_source,
                    0,
                    &submission_token.to_le_bytes(),
                );
                copy_pick_pixel(
                    &mut pick_encoder,
                    &res.picking,
                    &res.picking.staging_buffer,
                    x,
                    y,
                );
                pick_encoder.copy_buffer_to_buffer(
                    &res.submission_marker_source,
                    0,
                    &res.submission_marker_staging,
                    0,
                    4,
                );
                queue.submit(std::iter::once(pick_encoder.finish()));
                res.pick_attempt = Some(PickAttempt {
                    pixel: (x, y),
                    submission_token,
                    part_pick: pass == PickPass::Part,
                });
            } else if let Some((x, y)) = self.hover_request
                && res.hover.in_flight.is_none()
            {
                // Hover shares the click path's picking texture but not its
                // wait: the readback is collected on a later frame.
                let mut hover_encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("hover_readback_encoder"),
                    });
                render_picking(
                    &mut hover_encoder,
                    &res.pipelines,
                    &res.picking,
                    &res.meshes,
                    topology_meshes,
                );
                copy_pick_pixel(&mut hover_encoder, &res.picking, &res.hover.staging, x, y);
                queue.submit(std::iter::once(hover_encoder.finish()));
                res.hover.begin_readback();
            }
        }

        // ── Offscreen main viewport pass (with depth) ──
        // The egui paint callback's render pass has no depth attachment,
        // so the scene is rendered here with its own depth texture, then
        // blitted in paint().
        render_scene(encoder, &res.pipelines, &res.meshes, self.clear_colour);

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &eframe::egui_wgpu::CallbackResources,
    ) {
        let Some(res) = callback_resources.get::<ViewportResources>() else {
            return;
        };
        render_pass.set_pipeline(&res.pipelines.blit_pipeline);
        render_pass.set_bind_group(0, &res.pipelines.blit_bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

/// The offscreen target's clear colour for the theme's viewport background.
/// An sRGB target stores linear values and encodes on write; any other
/// target stores what it is given and the shader encodes its own output.
pub fn viewport_clear_colour(target_is_srgb: bool) -> wgpu::Color {
    let channel = |value: u8| {
        let gamma = f64::from(value) / 255.0;
        if target_is_srgb {
            if gamma <= 0.04045 {
                gamma / 12.92
            } else {
                ((gamma + 0.055) / 1.055).powf(2.4)
            }
        } else {
            gamma
        }
    };
    let colour = cadmark_ui::theme::VIEWPORT;
    wgpu::Color {
        r: channel(colour.r()),
        g: channel(colour.g()),
        b: channel(colour.b()),
        a: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use cadmark_core::geometry::{FaceId, TopologyElement};

    use super::*;

    #[test]
    fn submitted_pick_marker_permits_readback() {
        let attempt = PickAttempt {
            pixel: (438, 466),
            submission_token: 42,
            part_pick: false,
        };
        assert_eq!(
            pick_submission_decision(attempt, 42_u32.to_le_bytes()),
            PickSubmissionDecision::ReadPick
        );
    }

    #[test]
    fn stale_pick_marker_retries_the_exact_pixel() {
        let attempt = PickAttempt {
            pixel: (438, 466),
            submission_token: 42,
            part_pick: true,
        };
        assert_eq!(
            pick_submission_decision(attempt, 41_u32.to_le_bytes()),
            PickSubmissionDecision::Retry((438, 466), true)
        );
    }

    #[test]
    fn a_retried_part_pick_uses_the_part_picking_pass() {
        let retry = PickSubmissionDecision::Retry((438, 466), true);
        let PickSubmissionDecision::Retry(pixel, part_pick) = retry else {
            unreachable!()
        };

        assert_eq!(
            next_pick_request(None, None, Some((pixel, part_pick))),
            Some(((438, 466), PickPass::Part))
        );
    }

    #[test]
    fn incomplete_readback_retains_the_pick_anchor() {
        let mut in_flight = Some((120.0, 240.0));
        assert_eq!(
            completed_pick_transition(None, &mut in_flight),
            PickTransition::Waiting
        );
        assert_eq!(in_flight, Some((120.0, 240.0)));
    }

    #[test]
    fn completed_hit_consumes_and_returns_the_pick_anchor() {
        let element = TopologyElement::Face(FaceId(3));
        let mut in_flight = Some((120.0, 240.0));
        assert_eq!(
            completed_pick_transition(Some(CompletedPick::Hit(element.clone())), &mut in_flight),
            PickTransition::Hit(element, (120.0, 240.0))
        );
        assert_eq!(in_flight, None);
    }

    #[test]
    fn completed_background_clears_the_pick_anchor() {
        let mut in_flight = Some((120.0, 240.0));
        assert_eq!(
            completed_pick_transition(Some(CompletedPick::Background), &mut in_flight),
            PickTransition::Background
        );
        assert_eq!(in_flight, None);
    }

    #[test]
    fn viewport_clear_matches_the_theme_in_either_colour_space() {
        let linear = viewport_clear_colour(true);
        let gamma = viewport_clear_colour(false);
        // Linear values are darker than their gamma encoding for a dark grey.
        assert!(linear.r < gamma.r);
        let theme = cadmark_ui::theme::VIEWPORT;
        assert!((gamma.r - f64::from(theme.r()) / 255.0).abs() < 1e-9);
        assert!((gamma.g - f64::from(theme.g()) / 255.0).abs() < 1e-9);
        assert!((gamma.b - f64::from(theme.b()) / 255.0).abs() < 1e-9);
    }
}
