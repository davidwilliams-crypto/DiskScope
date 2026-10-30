#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(feature = "gui")]
mod app;
#[cfg(feature = "gui")]
mod theme;

use diskscope::cli::{self, Command};
use std::ffi::OsString;

/// The release build uses the Windows GUI subsystem, which starts without a
/// console. Attaching to the parent console lets `--help`, `--version`, scan
/// JSON and error text reach a terminal that launched the executable. Without
/// a parent console the call fails harmlessly and output stays discarded.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: AttachConsole takes a process id constant and touches no memory
    // owned by this program.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
#[cfg(not(windows))]
fn attach_parent_console() {}

fn startup_error(message: &str, gui: bool, code: i32) -> ! {
    eprintln!("{message}");
    #[cfg(all(windows, feature = "gui"))]
    if gui {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let message: Vec<u16> = format!("{message}\n\nFor background diagnostics, use --scan PATH --json NEW_REPORT.json. No profile log was written.").encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "DiskScope could not start"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: Both buffers are NUL-terminated and live through the call.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    let _ = gui;
    std::process::exit(code)
}

fn main() {
    attach_parent_console();
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let command = match cli::parse_args(&arguments) {
        Ok(command) => command,
        Err(error) => startup_error(
            &format!("{error}\nUse --help for usage."),
            false,
            cli::EXIT_ARGUMENT_ERROR,
        ),
    };
    let gui_command = matches!(&command, Command::Gui(_) | Command::GuiSession { .. });
    let managed_policy = std::env::current_exe()
        .map_err(|e| format!("Cannot locate executable for session policy: {e}"))
        .and_then(|exe| diskscope::session::SessionPolicy::beside_executable(&exe));
    let managed_policy = match managed_policy {
        Ok(policy) => policy,
        Err(error) => {
            startup_error(&error, gui_command, cli::EXIT_ARGUMENT_ERROR);
        }
    };
    let (initial_path, policy) = match command {
        Command::Help => {
            println!("{}", cli::help_text());
            return;
        }
        Command::Version => {
            println!("DiskScope {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Command::Scan(options) => {
            if managed_policy.helpdesk && options.backend != diskscope::model::BackendChoice::Normal
            {
                eprintln!("Helpdesk policy requires --backend normal");
                std::process::exit(cli::EXIT_ARGUMENT_ERROR);
            }
            match cli::run_scan_command_with_policy(&options, managed_policy) {
                Ok(code) => std::process::exit(code),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(cli::EXIT_FAILURE)
                }
            }
        }
        Command::Gui(path) => (path, managed_policy),
        Command::GuiSession { root, policy } => (root, managed_policy.merge(policy)),
    };
    #[cfg(feature = "gui")]
    {
        let root = initial_path;
        let access_mode = app::AccessMode::from_probe(diskscope::admin::is_elevated());
        let mut options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_title(format!("DiskScope {}", env!("CARGO_PKG_VERSION")))
                .with_inner_size([1440.0, 900.0])
                .with_min_inner_size([900.0, 640.0]),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        };
        access_mode.configure_native_options(&mut options);
        app::configure_session_options(policy, &mut options);
        if let Err(error) = eframe::run_native(
            "DiskScope",
            options,
            Box::new(move |cc| {
                Ok(Box::new(app::DiskScopeApp::new_session(
                    cc,
                    root,
                    access_mode,
                    policy,
                )))
            }),
        ) {
            startup_error(
                &format!("DiskScope could not start: {error}"),
                true,
                cli::EXIT_FAILURE,
            );
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = (initial_path, policy);
        eprintln!("Build with the gui feature to launch DiskScope.");
        std::process::exit(cli::EXIT_ARGUMENT_ERROR)
    }
}
