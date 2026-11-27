//! Choosing between the graphics backends eframe can render with.
//!
//! DiskScope compiles both of eframe's native backends and picks one when it
//! starts. `wgpu` renders through Direct3D 12 by default; `glow` renders through
//! OpenGL. The first backend to open a window wins. If it cannot, the other gets
//! one attempt, so the program runs wherever either graphics API works.
use eframe::{Error, Renderer};
use std::ffi::OsStr;

/// Environment variable that pins the backend, for a machine where the
/// automatic choice misbehaves. Values: `wgpu` or `glow`.
pub(crate) const RENDERER_VARIABLE: &str = "DISKSCOPE_RENDERER";

/// Backends to try, in order.
///
/// wgpu comes first because it does not need OpenGL, which some remote-desktop
/// sessions and virtual machines do not offer, while Direct3D is present
/// wherever Windows 10 is. `requested` is the value of
/// [`RENDERER_VARIABLE`]: a recognised value leaves out the fallback, because
/// that backend was asked for specifically.
pub(crate) fn backends(requested: Option<&OsStr>) -> Result<Vec<Renderer>, String> {
    let requested = requested
        .map(|value| value.to_string_lossy().trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    match requested.as_deref() {
        None => Ok(vec![Renderer::Wgpu, Renderer::Glow]),
        Some("wgpu") => Ok(vec![Renderer::Wgpu]),
        Some("glow") => Ok(vec![Renderer::Glow]),
        Some(other) => Err(format!(
            "{RENDERER_VARIABLE} must be wgpu or glow, not {other:?}."
        )),
    }
}

/// The wgpu settings for one start attempt: which backends to consider and
/// which adapter to prefer.
pub(crate) struct WgpuSettings {
    pub backends: eframe::egui_wgpu::wgpu::Backends,
    pub power_preference: eframe::egui_wgpu::wgpu::PowerPreference,
}

/// Direct3D 12 is present wherever Windows 10 is, including as a software
/// adapter in remote-desktop sessions and virtual machines, so it is the only
/// backend tried unless the user asks for another through wgpu's own
/// `WGPU_BACKEND` variable. Vulkan is left to that explicit request: some
/// installs have a broken Vulkan loader, and given the choice wgpu prefers
/// Vulkan over Direct3D. A disk-usage analyzer has no use for a discrete GPU,
/// so the low-power adapter is preferred unless `WGPU_POWER_PREF` says else.
pub(crate) fn wgpu_settings(
    backends_from_env: Option<eframe::egui_wgpu::wgpu::Backends>,
    power_from_env: Option<eframe::egui_wgpu::wgpu::PowerPreference>,
) -> WgpuSettings {
    use eframe::egui_wgpu::wgpu;
    WgpuSettings {
        backends: backends_from_env.unwrap_or(wgpu::Backends::DX12),
        power_preference: power_from_env.unwrap_or(wgpu::PowerPreference::LowPower),
    }
}

/// Prepares the window options for one attempt with `renderer`.
pub(crate) fn configure(renderer: Renderer, options: &mut eframe::NativeOptions) {
    // With `run_and_return` a backend that cannot start comes back as an error
    // for the caller to act on. Without it eframe exits the process
    // successfully instead, which would end the fallback before it begins.
    options.run_and_return = true;
    if renderer != Renderer::Wgpu {
        return;
    }
    use eframe::egui_wgpu::{WgpuSetup, wgpu};
    let WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup else {
        return;
    };
    let settings = wgpu_settings(
        wgpu::Backends::from_env(),
        wgpu::PowerPreference::from_env(),
    );
    setup.instance_descriptor.backends = settings.backends;
    setup.power_preference = settings.power_preference;
}

/// The graphics API a wgpu adapter renders through, for messages.
pub(crate) fn wgpu_api(backend: eframe::egui_wgpu::wgpu::Backend) -> &'static str {
    use eframe::egui_wgpu::wgpu::Backend;
    match backend {
        Backend::Dx12 => "Direct3D 12",
        Backend::Vulkan => "Vulkan",
        Backend::Gl => "OpenGL",
        Backend::Metal => "Metal",
        Backend::BrowserWebGpu => "WebGPU",
        Backend::Noop => "no output",
    }
}

/// The name of the backend that is rendering this session, for the Settings
/// window. Read once from the creation context: the adapter wgpu actually
/// picked, not the list of backends it was willing to try.
pub(crate) fn active(cc: &eframe::CreationContext<'_>) -> String {
    match &cc.wgpu_render_state {
        Some(state) => {
            let adapter = state.adapter.get_info();
            format!("{} (wgpu), {}", wgpu_api(adapter.backend), adapter.name)
        }
        None => "OpenGL 2.0 (glow)".to_owned(),
    }
}

/// The graphics API a backend tries by default, for messages.
pub(crate) fn describes(renderer: Renderer) -> &'static str {
    match renderer {
        Renderer::Wgpu => "wgpu (Direct3D 12 unless WGPU_BACKEND names another)",
        Renderer::Glow => "OpenGL 2.0 (glow)",
    }
}

/// Whether a start that failed is the backend's own doing, so the other backend
/// may still succeed. The application failing to be built, or Windows failing
/// to provide a window, would fail the same way again.
pub(crate) fn failure_belongs_to_backend(error: &Error) -> bool {
    matches!(
        error,
        Error::Wgpu(_) | Error::Glutin(_) | Error::NoGlutinConfigs(..) | Error::OpenGL(_)
    )
}

/// The message for a start that no backend survived, naming every attempt.
///
/// Graphics advice is only useful when the last failure was the backend's own.
/// If the application or Windows failed instead, that failure leads and the
/// backend attempts that preceded it are reported as background.
pub(crate) fn startup_failure(failures: &[(Renderer, Error)]) -> String {
    let Some((_, last)) = failures.last() else {
        return "DiskScope could not start: no graphics backend was tried.".to_owned();
    };
    if !failure_belongs_to_backend(last) {
        let mut message = format!("DiskScope could not start: {last}");
        for (renderer, error) in &failures[..failures.len() - 1] {
            message.push_str(&format!(
                "\n\nBefore that, the {} graphics backend could not start: {error}",
                describes(*renderer)
            ));
        }
        return message;
    }
    let mut message =
        String::from("DiskScope could not start: no graphics backend could open a window.");
    for (renderer, error) in failures {
        message.push_str(&format!("\n\n{}: {error}", describes(*renderer)));
    }
    message.push_str(
        "\n\nUpdate the graphics driver, or turn on hardware acceleration for this session. To force one backend, set DISKSCOPE_RENDERER to wgpu or glow.",
    );
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui_wgpu::wgpu;

    fn requested(value: &str) -> Option<&OsStr> {
        Some(OsStr::new(value))
    }

    #[test]
    fn without_a_request_wgpu_is_tried_first_and_opengl_is_the_fallback() {
        assert_eq!(
            backends(None).unwrap(),
            vec![Renderer::Wgpu, Renderer::Glow]
        );
        // An empty or blank value counts as no request.
        assert_eq!(backends(requested("")).unwrap(), backends(None).unwrap());
        assert_eq!(backends(requested("  ")).unwrap(), backends(None).unwrap());
    }

    #[test]
    fn a_recognised_request_pins_one_backend_without_a_fallback() {
        assert_eq!(backends(requested("wgpu")).unwrap(), vec![Renderer::Wgpu]);
        assert_eq!(backends(requested(" Glow ")).unwrap(), vec![Renderer::Glow]);
    }

    #[test]
    fn an_unknown_request_is_an_argument_error_that_names_the_variable() {
        // "opengl" is deliberately not an alias: wgpu's own WGPU_BACKEND uses
        // that word for a different thing.
        for value in ["directx", "opengl", "d3d12"] {
            let error = backends(requested(value)).unwrap_err();
            assert!(error.contains(RENDERER_VARIABLE), "{value}: {error}");
            assert!(error.contains("wgpu") && error.contains("glow"), "{error}");
        }
    }

    #[test]
    fn wgpu_tries_direct3d_12_on_the_low_power_adapter_unless_asked_otherwise() {
        let default = wgpu_settings(None, None);
        assert_eq!(default.backends, wgpu::Backends::DX12);
        assert_eq!(default.power_preference, wgpu::PowerPreference::LowPower);

        let asked = wgpu_settings(
            Some(wgpu::Backends::VULKAN),
            Some(wgpu::PowerPreference::HighPerformance),
        );
        assert_eq!(asked.backends, wgpu::Backends::VULKAN);
        assert_eq!(
            asked.power_preference,
            wgpu::PowerPreference::HighPerformance
        );
    }

    #[test]
    fn configure_prepares_each_attempt_and_leaves_the_opengl_options_alone() {
        // wgpu's own variables would override the defaults under test.
        if wgpu::Backends::from_env().is_some() || wgpu::PowerPreference::from_env().is_some() {
            return;
        }
        let mut options = eframe::NativeOptions::default();
        configure(Renderer::Glow, &mut options);
        assert!(
            options.run_and_return,
            "a failed start must come back as an error"
        );
        let eframe::egui_wgpu::WgpuSetup::CreateNew(glow_setup) = &options.wgpu_options.wgpu_setup
        else {
            panic!("eframe's default wgpu setup is CreateNew");
        };
        assert_eq!(
            glow_setup.power_preference,
            wgpu::PowerPreference::HighPerformance
        );

        configure(Renderer::Wgpu, &mut options);
        assert!(options.run_and_return);
        let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &options.wgpu_options.wgpu_setup
        else {
            panic!("still CreateNew");
        };
        assert_eq!(setup.instance_descriptor.backends, wgpu::Backends::DX12);
        assert_eq!(setup.power_preference, wgpu::PowerPreference::LowPower);
    }

    #[test]
    fn each_wgpu_adapter_backend_is_named_after_its_graphics_api() {
        let names = [
            wgpu_api(wgpu::Backend::Dx12),
            wgpu_api(wgpu::Backend::Vulkan),
            wgpu_api(wgpu::Backend::Gl),
            wgpu_api(wgpu::Backend::Metal),
            wgpu_api(wgpu::Backend::BrowserWebGpu),
            wgpu_api(wgpu::Backend::Noop),
        ];
        assert_eq!(
            names,
            [
                "Direct3D 12",
                "Vulkan",
                "OpenGL",
                "Metal",
                "WebGPU",
                "no output"
            ]
        );
    }

    #[test]
    fn a_kittest_context_without_a_renderer_is_reported_as_opengl() {
        // The headless test context has no wgpu state, which is what the glow
        // path looks like too.
        let cc = eframe::CreationContext::_new_kittest(eframe::egui::Context::default());
        assert!(cc.wgpu_render_state.is_none());
        assert_eq!(active(&cc), "OpenGL 2.0 (glow)");
    }

    #[test]
    fn only_the_backend_own_failures_are_worth_another_backend() {
        let wgpu_surface = Error::Wgpu(eframe::egui_wgpu::WgpuError::NoSurfaceFormatsAvailable);
        let wgpu_adapter = Error::Wgpu(
            eframe::egui_wgpu::WgpuError::CustomNativeAdapterSelectionError("no adapter".into()),
        );
        let opengl = Error::OpenGL(String::from("egui_glow requires opengl 2.0+").into());
        assert!(failure_belongs_to_backend(&wgpu_surface));
        assert!(failure_belongs_to_backend(&wgpu_adapter));
        assert!(failure_belongs_to_backend(&opengl));
        let app = Error::AppCreation("the app could not be built".into());
        assert!(!failure_belongs_to_backend(&app));
    }

    #[test]
    fn a_backend_failure_names_every_attempt_and_the_way_out() {
        let failures = vec![
            (
                Renderer::Wgpu,
                Error::Wgpu(eframe::egui_wgpu::WgpuError::NoSurfaceFormatsAvailable),
            ),
            (
                Renderer::Glow,
                Error::OpenGL(String::from("requires opengl 2.0+").into()),
            ),
        ];
        let message = startup_failure(&failures);
        let wgpu_at = message.find(describes(Renderer::Wgpu)).unwrap();
        let glow_at = message.find(describes(Renderer::Glow)).unwrap();
        assert!(
            wgpu_at < glow_at,
            "attempts are reported in the order tried"
        );
        assert!(message.contains("no graphics backend could open a window"));
        assert!(message.contains(RENDERER_VARIABLE));
    }

    #[test]
    fn a_failure_outside_the_backends_leads_the_message_without_graphics_advice() {
        let failures = vec![
            (
                Renderer::Wgpu,
                Error::Wgpu(eframe::egui_wgpu::WgpuError::NoSurfaceFormatsAvailable),
            ),
            (
                Renderer::Glow,
                Error::AppCreation("history folder unreadable".into()),
            ),
        ];
        let message = startup_failure(&failures);
        assert!(
            message.starts_with("DiskScope could not start: app creation error: history folder"),
            "{message}"
        );
        assert!(message.contains("Before that, the wgpu"), "{message}");
        assert!(!message.contains(RENDERER_VARIABLE), "{message}");
        assert!(!message.contains("no graphics backend"), "{message}");
    }

    #[test]
    fn no_attempt_at_all_is_stated_plainly() {
        assert_eq!(
            startup_failure(&[]),
            "DiskScope could not start: no graphics backend was tried."
        );
    }
}
