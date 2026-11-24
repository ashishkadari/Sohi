//! The tray / head context menu (spec §4.8): items, ids and commands.

use sohi_win::MenuItem;

/// What a menu choice asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    /// Start a voice turn.
    Talk,
    /// Open text mode.
    Type,
    /// Toggle forced offline.
    ForceOffline,
    /// Open `config.toml`.
    OpenConfig,
    /// Open the log folder.
    OpenLogFolder,
    /// Toggle "Start with Windows" (P8).
    StartWithWindows,
    /// Exit Sohi.
    Quit,
}

const COMMANDS: [MenuCommand; 7] = [
    MenuCommand::Talk,
    MenuCommand::Type,
    MenuCommand::ForceOffline,
    MenuCommand::OpenConfig,
    MenuCommand::OpenLogFolder,
    MenuCommand::StartWithWindows,
    MenuCommand::Quit,
];

impl MenuCommand {
    /// Menu id (non-zero; 0 means "dismissed").
    fn id(self) -> u16 {
        COMMANDS
            .iter()
            .position(|&c| c == self)
            .and_then(|i| u16::try_from(i + 1).ok())
            .unwrap_or(0)
    }

    /// The command for a menu id returned by `show_menu`.
    pub fn from_id(id: u16) -> Option<Self> {
        COMMANDS.get(usize::from(id).checked_sub(1)?).copied()
    }
}

/// Toggle states shown as check marks.
#[derive(Debug, Clone, Copy, Default)]
pub struct MenuState {
    /// Forced offline is on.
    pub force_offline: bool,
    /// Sohi starts with Windows.
    pub start_with_windows: bool,
}

/// The menu, in spec order. Features that don't exist yet are greyed out.
pub fn items(state: MenuState) -> [MenuItem<'static>; 9] {
    let cmd = |c: MenuCommand, label, enabled, checked| MenuItem::Command {
        id: c.id(),
        label,
        enabled,
        checked,
    };
    [
        cmd(MenuCommand::Talk, "Talk", true, None),
        cmd(MenuCommand::Type, "Type", true, None),
        cmd(
            MenuCommand::ForceOffline,
            "Force offline",
            true,
            Some(state.force_offline),
        ),
        MenuItem::Separator,
        cmd(MenuCommand::OpenConfig, "Open config", true, None),
        cmd(MenuCommand::OpenLogFolder, "Open log folder", true, None),
        cmd(
            MenuCommand::StartWithWindows,
            "Start with Windows",
            false,
            Some(state.start_with_windows),
        ),
        MenuItem::Separator,
        cmd(MenuCommand::Quit, "Quit", true, None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_zero_is_dismissed() {
        for c in COMMANDS {
            assert_ne!(c.id(), 0);
            assert_eq!(MenuCommand::from_id(c.id()), Some(c));
        }
        assert_eq!(MenuCommand::from_id(0), None);
        assert_eq!(MenuCommand::from_id(99), None);
    }

    #[test]
    fn every_command_appears_once() {
        let ids: Vec<u16> = items(MenuState::default())
            .iter()
            .filter_map(|i| match i {
                MenuItem::Command { id, .. } => Some(*id),
                MenuItem::Separator => None,
            })
            .collect();
        assert_eq!(ids.len(), COMMANDS.len());
        for c in COMMANDS {
            assert_eq!(ids.iter().filter(|&&id| id == c.id()).count(), 1);
        }
    }

    #[test]
    fn only_implemented_commands_are_enabled() {
        let enabled: Vec<_> = items(MenuState::default())
            .iter()
            .filter_map(|i| match i {
                MenuItem::Command {
                    id, enabled: true, ..
                } => MenuCommand::from_id(*id),
                _ => None,
            })
            .collect();
        assert_eq!(
            enabled,
            [
                MenuCommand::Talk,
                MenuCommand::Type,
                MenuCommand::ForceOffline,
                MenuCommand::OpenConfig,
                MenuCommand::OpenLogFolder,
                MenuCommand::Quit
            ]
        );
    }
}
