//! User-interface components for the workspace.

mod annotation_toolbar;
mod command_palette;
mod notes_panel;
mod settings;
mod storage;
mod toolbars;
mod widgets;

pub(super) use command_palette::COMMAND_PALETTE_LABEL;
pub(super) use settings::SettingsTab;
pub(super) use toolbars::TopbarSlot;
pub(super) use widgets::{MENU_GLASS_ALPHA, truncate_filename};
