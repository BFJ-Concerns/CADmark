//! One wgpu device for every test that draws.
//!
//! The renderer's unit rung is proven on a software adapter, and a
//! machine's own GPU may be held by another process, on which the Vulkan
//! driver stops answering. So a test never asks the driver directly: it
//! takes the device from here, and this module tries a fixed list of
//! routes, each on its own thread under a deadline, and keeps the first
//! that answers with a device.
//!
//! With no operator override the order is: Vulkan's software adapter
//! (lavapipe), GL's software adapter (llvmpipe, with Mesa's EGL vendor
//! preferred so a proprietary vendor library does not hide it), Vulkan on
//! the machine's GPU, GL on the machine's GPU. `WGPU_BACKEND`, when set,
//! replaces that list with the named backend alone — its software adapter
//! first, then any — because an operator who named a backend meant it.
//!
//! One process acquires once and shares the device: a `wgpu::Device` is
//! a handle, and one GL context serving every test is what keeps parallel
//! tests from contending for the loader.

use std::fmt;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Once, OnceLock};
use std::thread;
use std::time::Duration;

/// A device for tests, with where it came from.
pub struct TestGpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::AdapterInfo,
    pub route: Route,
}

impl TestGpu {
    /// The device and its queue, cloned: both are reference-counted
    /// handles, so every test may hold its own pair.
    pub fn handles(&self) -> (wgpu::Device, wgpu::Queue) {
        (self.device.clone(), self.queue.clone())
    }
}

/// The shared device: a software adapter where the machine has one, the
/// machine's own GPU otherwise. Panics, naming every route tried and why
/// it failed, when nothing answers.
pub fn shared() -> &'static TestGpu {
    static SHARED: OnceLock<Result<TestGpu, String>> = OnceLock::new();
    settle(SHARED.get_or_init(|| acquire(Policy::PreferSoftware)))
}

/// A software adapter or nothing: for a proof specified on the software
/// adapter, where the machine's GPU would be a different instrument.
pub fn software() -> &'static TestGpu {
    static SOFTWARE: OnceLock<Result<TestGpu, String>> = OnceLock::new();
    settle(SOFTWARE.get_or_init(|| acquire(Policy::SoftwareOnly)))
}

fn settle(outcome: &'static Result<TestGpu, String>) -> &'static TestGpu {
    match outcome {
        Ok(gpu) => gpu,
        Err(report) => panic!("{report}"),
    }
}

/// Whether the machine's own GPU may be taken when no software adapter
/// answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    PreferSoftware,
    SoftwareOnly,
}

/// One way of asking for an adapter: which backends, and whether the
/// software adapter is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The backend `WGPU_BACKEND` names, software adapter required.
    OperatorSoftware,
    /// The backend `WGPU_BACKEND` names, any adapter.
    OperatorAny,
    /// Vulkan's software adapter: lavapipe.
    VulkanSoftware,
    /// GL's software adapter: llvmpipe.
    GlSoftware,
    /// Vulkan on the machine's own GPU.
    VulkanHardware,
    /// GL on the machine's own GPU.
    GlHardware,
}

impl Route {
    fn software_only(self) -> bool {
        matches!(
            self,
            Route::OperatorSoftware | Route::VulkanSoftware | Route::GlSoftware
        )
    }

    /// How long a route may take to answer before the next is tried. A
    /// software adapter initialises in well under a second; a driver that
    /// has not answered in this long is the stall this module exists to
    /// step around, not a slow success.
    fn deadline(self) -> Duration {
        if self.software_only() {
            Duration::from_secs(10)
        } else {
            Duration::from_secs(20)
        }
    }

    fn instance(self) -> wgpu::InstanceDescriptor {
        let backends = match self {
            Route::OperatorSoftware | Route::OperatorAny => {
                return wgpu::InstanceDescriptor::from_env_or_default();
            }
            Route::VulkanSoftware | Route::VulkanHardware => wgpu::Backends::VULKAN,
            Route::GlSoftware | Route::GlHardware => wgpu::Backends::GL,
        };
        wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        }
    }

    /// Ask this route for a device. Blocks for as long as the driver
    /// takes, which is why callers run it under [`within`].
    fn acquire(self) -> Result<TestGpu, String> {
        let instance = wgpu::Instance::new(&self.instance());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: self.software_only(),
            compatible_surface: None,
        }))
        .ok_or_else(|| "no adapter".to_string())?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("cadmark test device"),
                required_features: wgpu::Features::empty(),
                // The downlevel floor every adapter meets, with the
                // adapter's own texture sizes, so a render at viewport
                // resolution is not refused by a limit set below it.
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
            },
            None,
        ))
        .map_err(|error| {
            format!(
                "adapter {} gave no device: {error}",
                adapter.get_info().name
            )
        })?;
        Ok(TestGpu {
            device,
            queue,
            adapter: adapter.get_info(),
            route: self,
        })
    }
}

impl fmt::Display for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Route::OperatorSoftware => "WGPU_BACKEND's software adapter",
            Route::OperatorAny => "WGPU_BACKEND's adapter",
            Route::VulkanSoftware => "Vulkan software adapter (lavapipe)",
            Route::GlSoftware => "GL software adapter (llvmpipe)",
            Route::VulkanHardware => "Vulkan on the machine's GPU",
            Route::GlHardware => "GL on the machine's GPU",
        })
    }
}

/// The routes a policy tries, in order. `operator_backend` is the value
/// of `WGPU_BACKEND` when the operator set one.
pub fn routes(policy: Policy, operator_backend: Option<&str>) -> Vec<Route> {
    let all = match operator_backend {
        Some(_) => vec![Route::OperatorSoftware, Route::OperatorAny],
        None => vec![
            Route::VulkanSoftware,
            Route::GlSoftware,
            Route::VulkanHardware,
            Route::GlHardware,
        ],
    };
    match policy {
        Policy::PreferSoftware => all,
        Policy::SoftwareOnly => all
            .into_iter()
            .filter(|route| route.software_only())
            .collect(),
    }
}

fn acquire(policy: Policy) -> Result<TestGpu, String> {
    prefer_mesa_egl();
    let operator = std::env::var("WGPU_BACKEND").ok();
    let mut report = Vec::new();
    for route in routes(policy, operator.as_deref()) {
        match within(route.deadline(), &route.to_string(), move || {
            route.acquire()
        }) {
            Ok(Ok(gpu)) => {
                eprintln!("test device: {} through {route}", gpu.adapter.name);
                return Ok(gpu);
            }
            Ok(Err(reason)) => report.push(format!("{route}: {reason}")),
            Err(Interrupted::TimedOut) => {
                eprintln!(
                    "test device: {route} did not answer within {:?}; trying the next route",
                    route.deadline()
                );
                report.push(format!("{route}: no answer within {:?}", route.deadline()));
            }
            Err(Interrupted::Died) => report.push(format!("{route}: the driver call panicked")),
        }
    }
    Err(format!(
        "no wgpu device for the tests — {}. Install a software rasteriser (Mesa's lavapipe \
         for Vulkan, or llvmpipe for GL), or set WGPU_BACKEND to a backend that answers.",
        report.join("; ")
    ))
}

/// Why a route produced no answer at all.
#[derive(Debug, PartialEq, Eq)]
pub enum Interrupted {
    /// The deadline passed. The thread is left running: it is inside a
    /// driver call that cannot be interrupted, and the process is a test
    /// binary that ends soon anyway.
    TimedOut,
    /// The work panicked before answering.
    Died,
}

/// Run `work` on its own thread and wait at most `deadline` for its answer.
pub fn within<T: Send + 'static>(
    deadline: Duration,
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Interrupted> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name(format!("test device: {name}"))
        .spawn(move || {
            // A dropped sender is how a panic reports itself.
            let _ = sender.send(work());
        })
        .expect("a thread for the adapter request");
    match receiver.recv_timeout(deadline) {
        Ok(answer) => Ok(answer),
        Err(RecvTimeoutError::Timeout) => Err(Interrupted::TimedOut),
        Err(RecvTimeoutError::Disconnected) => Err(Interrupted::Died),
    }
}

/// Point the GL loader at Mesa, once, before any driver opens.
///
/// wgpu can only force a fallback adapter when the loader offers a CPU
/// one. Where a proprietary vendor's EGL library is installed it answers
/// first and reports that vendor's GPU, so no fallback exists in the list
/// to force; naming Mesa's EGL vendor file instead puts llvmpipe back in
/// it. On a machine with no such library the loader already resolves to
/// Mesa, where this is redundant rather than wrong, and a value the
/// operator set is left alone.
fn prefer_mesa_egl() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        const VENDOR_VARIABLE: &str = "__EGL_VENDOR_LIBRARY_FILENAMES";
        const MESA_VENDOR_FILES: [&str; 2] = [
            "/usr/share/glvnd/egl_vendor.d/50_mesa.json",
            "/etc/glvnd/egl_vendor.d/50_mesa.json",
        ];
        if std::env::var_os(VENDOR_VARIABLE).is_some() {
            return;
        }
        let Some(mesa) = MESA_VENDOR_FILES
            .iter()
            .find(|path| std::path::Path::new(path).exists())
        else {
            return;
        };
        // SAFETY: this runs before this process has opened any driver, so
        // no native code is reading the environment yet. Other test
        // threads may be live, but Rust's own environment access is
        // serialised by the standard library.
        unsafe { std::env::set_var(VENDOR_VARIABLE, mesa) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_adapters_come_before_the_machines_gpu() {
        assert_eq!(
            routes(Policy::PreferSoftware, None),
            [
                Route::VulkanSoftware,
                Route::GlSoftware,
                Route::VulkanHardware,
                Route::GlHardware,
            ]
        );
        assert_eq!(
            routes(Policy::SoftwareOnly, None),
            [Route::VulkanSoftware, Route::GlSoftware]
        );
    }

    #[test]
    fn an_operator_backend_replaces_the_list() {
        assert_eq!(
            routes(Policy::PreferSoftware, Some("gl")),
            [Route::OperatorSoftware, Route::OperatorAny]
        );
        assert_eq!(
            routes(Policy::SoftwareOnly, Some("gl")),
            [Route::OperatorSoftware]
        );
    }

    #[test]
    fn a_route_that_does_not_answer_is_left_behind_at_its_deadline() {
        let started = std::time::Instant::now();
        let outcome = within(Duration::from_millis(50), "stalled", || {
            thread::sleep(Duration::from_secs(5));
            1
        });
        assert_eq!(outcome, Err(Interrupted::TimedOut));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "waited on the stalled route"
        );
    }

    #[test]
    fn an_answer_and_a_panic_are_told_apart() {
        assert_eq!(within(Duration::from_secs(1), "quick", || 7), Ok(7));
        let died = within(Duration::from_secs(1), "panicking", || -> u8 {
            panic!("driver exploded")
        });
        assert_eq!(died, Err(Interrupted::Died));
    }

    #[test]
    fn the_shared_device_answers_and_is_one_device() {
        let first = shared();
        let second = shared();
        assert!(std::ptr::eq(first, second));
        assert!(!first.adapter.name.is_empty());
    }
}
