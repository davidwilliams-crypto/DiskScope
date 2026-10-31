//! Regression for disabled path completion when keyboard events share a frame.
use super::test_support::physical_app_frame;
use super::*;

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn focused_path_app(width: f32) -> (DiskScopeApp, egui::Context) {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = DiskScopeApp::new_session(
        &cc,
        None,
        AccessMode::Standard,
        SessionPolicy {
            helpdesk: true,
            no_history: true,
        },
    );
    app.path_text = std::env::temp_dir()
        .join(format!("DiskScope-modal-missing-{}", std::process::id()))
        .to_string_lossy()
        .into_owned();
    app.path_edited = true;
    let size = egui::vec2(width, 900.0);
    for _ in 0..3 {
        physical_app_frame(&mut app, &ctx, size, 1.0, vec![]);
    }
    assert_eq!(investigation::compact(&ctx), width < 1100.0);
    for _ in 0..60 {
        let output = physical_app_frame(
            &mut app,
            &ctx,
            size,
            1.0,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
        );
        let update = output.platform_output.accesskit_update.unwrap();
        if update
            .nodes
            .iter()
            .any(|(id, node)| *id == update.focus && node.value() == Some(app.path_text.as_str()))
        {
            return (app, ctx);
        }
        physical_app_frame(&mut app, &ctx, size, 1.0, vec![]);
    }
    panic!("edited scan-path field must be keyboard reachable at width {width}");
}

fn assert_modal_enter_preserves_scan(width: f32) {
    let (mut app, ctx) = focused_path_app(width);
    let generation = app.generation;
    let root = app.root_path.clone();
    physical_app_frame(
        &mut app,
        &ctx,
        egui::vec2(width, 900.0),
        1.0,
        vec![
            key(egui::Key::K, egui::Modifiers::CTRL),
            key(egui::Key::Enter, egui::Modifiers::NONE),
        ],
    );
    assert_eq!(
        app.generation, generation,
        "a disabled path field must not start a scan behind Commands"
    );
    assert_eq!(
        app.root_path, root,
        "a modal must preserve the investigation root"
    );
}

#[test]
fn commands_opening_with_enter_does_not_scan_from_wide_path_field() {
    assert_modal_enter_preserves_scan(1440.0);
}

#[test]
fn commands_opening_with_enter_does_not_scan_from_compact_path_field() {
    assert_modal_enter_preserves_scan(1000.0);
}

#[test]
fn enter_still_starts_a_scan_from_enabled_path_fields() {
    for width in [1440.0, 1000.0] {
        let (mut app, ctx) = focused_path_app(width);
        let generation = app.generation;
        let requested = PathBuf::from(&app.path_text);
        physical_app_frame(
            &mut app,
            &ctx,
            egui::vec2(width, 900.0),
            1.0,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(app.generation, generation + 1);
        assert_eq!(app.root_path, requested);
    }
}
