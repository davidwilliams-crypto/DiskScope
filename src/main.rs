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

fn main() {
    attach_parent_console();
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let command = match cli::parse_args(&arguments) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}\nUse --help for usage.");
            std::process::exit(cli::EXIT_ARGUMENT_ERROR)
        }
    };
    let initial_path = match command {
        Command::Help => {
            println!("{}", cli::help_text());
            return;
        }
        Command::Version => {
            println!("DiskScope {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Command::Scan(options) => match cli::run_scan_command(&options) {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(cli::EXIT_FAILURE)
            }
        },
        Command::Gui(path) => path,
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
        if let Err(error) = eframe::run_native(
            "DiskScope",
            options,
            Box::new(move |cc| Ok(Box::new(app::DiskScopeApp::new(cc, root, access_mode)))),
        ) {
            eprintln!("DiskScope could not start: {error}");
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = initial_path;
        eprintln!("Build with the gui feature to launch DiskScope.");
        std::process::exit(cli::EXIT_ARGUMENT_ERROR)
    }
}
