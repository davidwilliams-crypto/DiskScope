//! Helpers shared by the kittest-based tests of several `app` submodules.
use super::*;
use eframe::App as _;

/// Model the integration's physical client size and native scaling before
/// app text zoom. Setting a zoomed UI's RawInput rect to physical pixels
/// would accidentally give the test a larger client than the real window.
pub(super) fn physical_app_frame(
    app: &mut DiskScopeApp,
    ctx: &egui::Context,
    physical_size: egui::Vec2,
    native_scale: f32,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let points = physical_size / (native_scale * ctx.zoom_factor());
    let mut input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, points)),
        events,
        ..Default::default()
    };
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(native_scale);
    let output = ctx.run_ui(input, |ui| app.ui(ui, &mut eframe::Frame::_new_kittest()));
    assert_accesskit_focus_is_reachable(ctx, &output);
    output
}

/// egui emits a complete TreeUpdate each pass. Check reachability through the
/// current root's children, so a detached or removed node cannot receive focus.
pub(super) fn assert_accesskit_focus_is_reachable(ctx: &egui::Context, output: &egui::FullOutput) {
    let Some(update) = &output.platform_output.accesskit_update else {
        return;
    };
    let mut pending = vec![update.tree.as_ref().expect("egui emits the tree root").root];
    let mut reachable = std::collections::HashSet::new();
    while let Some(id) = pending.pop() {
        if !reachable.insert(id) {
            continue;
        }
        let (_, node) = update
            .nodes
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .unwrap_or_else(|| panic!("accessibility tree references absent node {id:?}"));
        pending.extend(node.children());
    }
    assert!(
        reachable.contains(&update.focus),
        "accessibility focus {:?} must be reachable from the live root",
        update.focus
    );
    struct Changes;
    impl accesskit_consumer::TreeChangeHandler for Changes {
        fn node_added(&mut self, _: &accesskit_consumer::Node) {}
        fn node_updated(&mut self, _: &accesskit_consumer::Node, _: &accesskit_consumer::Node) {}
        fn focus_moved(
            &mut self,
            _: Option<&accesskit_consumer::Node>,
            _: Option<&accesskit_consumer::Node>,
        ) {
        }
        fn node_removed(&mut self, _: &accesskit_consumer::Node) {}
    }
    type Consumer = Arc<std::sync::Mutex<Option<accesskit_consumer::Tree>>>;
    let consumer: Consumer = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Consumer>(egui::Id::new("test_accesskit_consumer"))
            .clone()
    });
    let mut consumer = consumer.lock().unwrap();
    if let Some(tree) = consumer.as_mut() {
        tree.update_and_process_changes(update.clone(), &mut Changes);
    } else {
        *consumer = Some(accesskit_consumer::Tree::new(update.clone(), true));
    }
}

/// Every piece of text a frame paints, in paint order.
pub(super) fn rendered_texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn collect(shape: &egui::epaint::Shape, texts: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
            egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().for_each(|shape| collect(shape, texts));
            }
            _ => {}
        }
    }
    let mut texts = Vec::new();
    shapes
        .iter()
        .for_each(|shape| collect(&shape.shape, &mut texts));
    texts
}

/// Runs `show` for a few frames so windows and tables finish their sizing
/// passes, and returns the last frame.
pub(super) fn settle_frames(
    ctx: &egui::Context,
    mut show: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    let mut output = egui::FullOutput::default();
    for _ in 0..3 {
        output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 900.0),
                )),
                ..Default::default()
            },
            &mut show,
        );
    }
    output
}
