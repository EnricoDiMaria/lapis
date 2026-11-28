//! User-interface components for the workspace.

mod notes_panel;
mod settings;
mod storage;
mod toolbars;
mod widgets;

pub(super) use settings::SettingsTab;
pub(super) use toolbars::TopbarSlot;
pub(super) use widgets::{MENU_GLASS_ALPHA, truncate_filename};
