#![forbid(unsafe_code)]
//! Bridges the GUI's repaint context to the backend-neutral
//! `ChangeNotifier`. `vibemux_client` has no egui dependency, so the
//! `ChangeNotifier for egui::Context` impl cannot live there (the orphan
//! rule needs a local type); this newtype is that local type.

use vibemux_client::change_notifier::ChangeNotifier;

pub(crate) struct EguiNotifier(pub egui::Context);

impl ChangeNotifier for EguiNotifier {
    fn notify(&self) {
        self.0.request_repaint();
    }
}
