//! Session-local evidence for the last file action. Logical reduction and
//! volume availability are deliberately independent measurements.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Presence {
    Present,
    Absent,
    Unknown,
}

pub(super) struct ActionReview {
    pub path: PathBuf,
    pub root: PathBuf,
    pub generation: u64,
    pub message: String,
    pub recycled: bool,
    pub before_logical: Option<u64>,
    pub before_backend: String,
    pub after_logical: Option<u64>,
    pub before_volume: Option<(native::VolumeSpace, std::time::SystemTime)>,
    pub after_volume: Option<(native::VolumeSpace, std::time::SystemTime)>,
    pub state: Option<ScanState>,
    pub presence: Option<Presence>,
    pub probe_started: bool,
    pub volume_refresh_requested: bool,
}

impl ActionReview {
    pub fn logical_change(&self) -> Option<i128> {
        self.after_logical
            .zip(self.before_logical)
            .map(|(after, before)| i128::from(after) - i128::from(before))
    }
    pub fn volume_change(&self) -> Option<i128> {
        let (before, before_time) = self.before_volume?;
        let (after, after_time) = self.after_volume?;
        (after_time >= before_time && before.total == after.total)
            .then_some(i128::from(after.available) - i128::from(before.available))
    }
    pub fn verification(&self) -> &'static str {
        match self.presence {
            Some(Presence::Absent) => "Path absence verified",
            Some(Presence::Present) => {
                "Path still exists; it may be pending deletion or a replacement"
            }
            Some(Presence::Unknown) => "Path verification unavailable",
            None => "Path verification pending",
        }
    }
}

pub(super) fn signed_bytes(value: i128) -> String {
    format!(
        "{}{}",
        if value < 0 { "−" } else { "+" },
        format_bytes(value.unsigned_abs().min(u128::from(u64::MAX)) as u64)
    )
}

impl DiskScopeApp {
    pub(super) fn poll_action_review(&mut self, ctx: &egui::Context) {
        let Some(review) = self.action_review.as_mut() else {
            return;
        };
        if review.generation != self.generation
            || review.root != self.root_path
            || self.snapshot.progress.state.is_active()
        {
            return;
        }
        if review.state.is_none() {
            review.state = Some(self.snapshot.progress.state);
            // Partial/failed/stopped discovery does not prove a size reduction.
            if self.snapshot.progress.state == ScanState::Complete
                && scan_history::compatible_backends(
                    &review.before_backend,
                    &self.snapshot.progress.backend,
                )
            {
                review.after_logical = Some(self.snapshot.progress.logical);
            }
        }
        if !review.volume_refresh_requested {
            review.volume_refresh_requested = self
                .volume_tx
                .try_send((self.generation, self.root_path.clone()))
                .is_ok();
        }
        if !review.probe_started {
            review.probe_started = true;
            let path = review.path.clone();
            let generation = review.generation;
            let tx = self.native_tx.clone();
            let ctx = ctx.clone();
            let spawn = std::thread::Builder::new()
                .name("action-presence".into())
                .spawn(move || {
                    let presence = match std::fs::symlink_metadata(&path) {
                        Ok(_) => Presence::Present,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            Presence::Absent
                        }
                        Err(_) => Presence::Unknown,
                    };
                    let _ = tx.send(NativeResult::ActionPresence {
                        generation,
                        path,
                        presence,
                    });
                    ctx.request_repaint();
                });
            if spawn.is_err() {
                review.presence = Some(Presence::Unknown);
            }
        }
    }

    pub(super) fn action_review_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        let Some(review) = self.action_review.as_ref() else {
            return;
        };
        let mut dismiss = false;
        let mut content = |ui: &mut egui::Ui| {
            ui.label(RichText::new(&review.message).strong());
            ui.label(review.path.to_string_lossy());
            ui.label(review.verification());
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "Scan logical change: {}",
                    review
                        .logical_change()
                        .map(signed_bytes)
                        .unwrap_or_else(|| "Unknown until a complete rescan".into())
                ));
                ui.label(format!(
                    "Observed available-space change: {}",
                    review
                        .volume_change()
                        .map(signed_bytes)
                        .unwrap_or_else(|| "Unknown / measurement pending".into())
                ));
            });
            if let Some(state) = review.state {
                ui.label(format!("Reconciliation: {}", state.label()));
            }
            if review.recycled {
                ui.label("Recycling normally keeps the storage occupied until the Recycle Bin is emptied.");
            }
            ui.label(RichText::new("Logical change is not reclaimed space. Other drive activity can affect available space.").small().color(p.muted));
            ui.collapsing("Measurement details", |ui| {
                ui.label(format!("Compared scan root: {}", review.root.display()));
                for (label, sample) in [
                    ("Before", review.before_volume),
                    ("After", review.after_volume),
                ] {
                    ui.label(match sample {
                        Some((space, time)) => format!(
                            "{label}: {} available · {} UTC",
                            format_bytes(space.available),
                            date_time_label(
                                time.duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs()
                            )
                        ),
                        None => format!("{label}: volume measurement unavailable"),
                    });
                }
            });
            dismiss = ui.button("Dismiss outcome").clicked();
        };
        if investigation::compact(ui.ctx()) {
            ui.menu_button("Action outcome · verification", |ui| {
                ui.set_max_width((ui.ctx().content_rect().width() - 60.0).clamp(200.0, 620.0));
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, &mut content);
            });
        } else {
            egui::CollapsingHeader::new("Action outcome · verification")
                .id_salt("action_outcome")
                .default_open(true)
                .show(ui, &mut content);
        }
        if dismiss {
            self.action_review = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn review() -> ActionReview {
        ActionReview {
            path: "fixture".into(),
            root: "root".into(),
            generation: 1,
            message: "Recycled".into(),
            recycled: true,
            before_logical: Some(100),
            before_backend: "Normal".into(),
            after_logical: Some(20),
            before_volume: Some((
                native::VolumeSpace {
                    available: 10,
                    total: 100,
                },
                std::time::UNIX_EPOCH,
            )),
            after_volume: Some((
                native::VolumeSpace {
                    available: 10,
                    total: 100,
                },
                std::time::UNIX_EPOCH,
            )),
            state: Some(ScanState::Complete),
            presence: Some(Presence::Absent),
            probe_started: true,
            volume_refresh_requested: false,
        }
    }
    #[test]
    fn logical_reduction_does_not_imply_reclaimed_space() {
        let review = review();
        assert_eq!(review.logical_change(), Some(-80));
        assert_eq!(review.volume_change(), Some(0));
        assert_eq!(review.verification(), "Path absence verified");
    }
    #[test]
    fn missing_or_incompatible_measurements_stay_unknown() {
        let mut review = review();
        review.after_logical = None;
        review.after_volume.as_mut().unwrap().0.total = 200;
        assert_eq!(review.logical_change(), None);
        assert_eq!(review.volume_change(), None);
        review.presence = Some(Presence::Unknown);
        assert_eq!(review.verification(), "Path verification unavailable");
    }

    #[test]
    fn incomplete_before_scan_cannot_prove_a_logical_change() {
        let mut review = review();
        review.before_logical = None;
        review.after_logical = Some(1000);
        assert_eq!(review.logical_change(), None);
    }

    #[test]
    fn reconciliation_with_a_different_backend_keeps_logical_change_unknown() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app =
            DiskScopeApp::with_history_directory(&cc, None, PathBuf::new(), AccessMode::Admin);
        app.root_path = "root".into();
        app.generation = 1;
        let progress = &mut Arc::make_mut(&mut app.snapshot).progress;
        progress.state = ScanState::Complete;
        progress.logical = 1000;
        progress.backend = "NTFS raw metadata".into();
        let mut evidence = review();
        evidence.state = None;
        evidence.after_logical = None;
        app.action_review = Some(evidence);
        app.poll_action_review(&ctx);
        assert_eq!(app.action_review.as_ref().unwrap().logical_change(), None);
    }
}
