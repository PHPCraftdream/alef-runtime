// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pure broker-tray logic: defaults, the fixed three-item menu layout and the
//! mapping from menu item ids to commands. No GUI, no channels — everything is
//! testable here.

use super::BrokerTrayCommand;

/// Stable menu ids shared by the native (muda) and Linux (dbusmenu) backends.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) const SHOW_APPS_ID: &str = "broker-show-apps";
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) const SHOW_SETTINGS_ID: &str = "broker-show-settings";
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) const QUIT_ID: &str = "broker-quit";
/// dbusmenu item ids: the root is 0, the three top-level items follow in display order.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const SHOW_APPS_ITEM: i32 = 1;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const SHOW_SETTINGS_ITEM: i32 = 2;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const QUIT_ITEM: i32 = 3;

pub(crate) const DEFAULT_TOOLTIP: &str = "Alef Broker";
pub(crate) const DEFAULT_SHOW_APPS: &str = "Apps";
pub(crate) const DEFAULT_SHOW_SETTINGS: &str = "Settings";
pub(crate) const DEFAULT_QUIT: &str = "Quit";

/// One menu entry as data: the backend-independent shape of the fixed menu.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) struct Entry {
    pub id: &'static str,
    pub item: i32,
    pub label: String,
}

/// The fixed menu: three enabled top-level items, Apps, Settings, Quit.
/// The separator between Settings and Quit is rendered by the platform backends.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn entries(show_apps: &str, show_settings: &str, quit: &str) -> [Entry; 3] {
    [
        Entry {
            id: SHOW_APPS_ID,
            item: SHOW_APPS_ITEM,
            label: show_apps.to_owned(),
        },
        Entry {
            id: SHOW_SETTINGS_ID,
            item: SHOW_SETTINGS_ITEM,
            label: show_settings.to_owned(),
        },
        Entry {
            id: QUIT_ID,
            item: QUIT_ITEM,
            label: quit.to_owned(),
        },
    ]
}

/// Native (muda) menu id to command.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn command(id: &str) -> Option<BrokerTrayCommand> {
    match id {
        SHOW_APPS_ID => Some(BrokerTrayCommand::ShowApps),
        SHOW_SETTINGS_ID => Some(BrokerTrayCommand::ShowSettings),
        QUIT_ID => Some(BrokerTrayCommand::Quit),
        _ => None,
    }
}

/// dbusmenu item id to command.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn command_item(item: i32) -> Option<BrokerTrayCommand> {
    match item {
        SHOW_APPS_ITEM => Some(BrokerTrayCommand::ShowApps),
        SHOW_SETTINGS_ITEM => Some(BrokerTrayCommand::ShowSettings),
        QUIT_ITEM => Some(BrokerTrayCommand::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_has_three_items_in_order() {
        let items = entries("Apps", "Settings", "Quit");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, "broker-show-apps");
        assert_eq!(items[0].item, 1);
        assert_eq!(items[0].label, "Apps");
        assert_eq!(items[1].id, "broker-show-settings");
        assert_eq!(items[1].item, 2);
        assert_eq!(items[1].label, "Settings");
        assert_eq!(items[2].id, "broker-quit");
        assert_eq!(items[2].item, 3);
        assert_eq!(items[2].label, "Quit");
    }

    #[test]
    fn menu_keeps_custom_labels() {
        let items = entries("Apps", "Settings", "Quit");
        assert_eq!(items[0].label, "Apps");
        assert_eq!(items[1].label, "Settings");
        assert_eq!(items[2].label, "Quit");
    }

    #[test]
    fn native_ids_map_to_commands() {
        assert_eq!(
            command("broker-show-apps"),
            Some(BrokerTrayCommand::ShowApps)
        );
        assert_eq!(
            command("broker-show-settings"),
            Some(BrokerTrayCommand::ShowSettings)
        );
        assert_eq!(command("broker-quit"), Some(BrokerTrayCommand::Quit));
        assert_eq!(command("other"), None);
        assert_eq!(command(""), None);
    }

    #[test]
    fn dbusmenu_ids_map_to_commands() {
        assert_eq!(command_item(1), Some(BrokerTrayCommand::ShowApps));
        assert_eq!(command_item(2), Some(BrokerTrayCommand::ShowSettings));
        assert_eq!(command_item(3), Some(BrokerTrayCommand::Quit));
        assert_eq!(command_item(0), None); // the root
        assert_eq!(command_item(4), None);
    }
}
