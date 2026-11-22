//! Hotkey strings from the config (`"Ctrl+Alt+Space"`), parsed into
//! modifiers and a Windows virtual-key code (spec §4.7, §5.7).

use std::fmt;

/// A global hotkey: at least one modifier plus one key.
#[allow(clippy::struct_excessive_bools)] // four independent modifier keys
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    /// Ctrl held.
    pub ctrl: bool,
    /// Alt held.
    pub alt: bool,
    /// Shift held.
    pub shift: bool,
    /// Windows key held.
    pub win: bool,
    /// Windows virtual-key code of the main key.
    pub vk: u32,
}

/// Why a hotkey string was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyError {
    /// The string is empty.
    Empty,
    /// A part is not a modifier or a known key.
    UnknownKey(String),
    /// More than one non-modifier key.
    TwoKeys,
    /// No non-modifier key.
    NoKey,
    /// No modifier: a bare key would steal normal typing.
    NoModifier,
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty hotkey"),
            Self::UnknownKey(k) => write!(f, "unknown key \"{k}\""),
            Self::TwoKeys => f.write_str("more than one key"),
            Self::NoKey => f.write_str("no key after the modifiers"),
            Self::NoModifier => f.write_str("needs Ctrl, Alt, Shift or Win"),
        }
    }
}

impl std::error::Error for HotkeyError {}

impl Hotkey {
    /// Parses `"Ctrl+Alt+Space"`-style text. Case-insensitive; parts are
    /// separated by `+` with optional spaces.
    ///
    /// # Errors
    /// Returns [`HotkeyError`] if the text is not a modifier set plus
    /// exactly one known key.
    pub fn parse(text: &str) -> Result<Self, HotkeyError> {
        if text.trim().is_empty() {
            return Err(HotkeyError::Empty);
        }
        let mut hotkey = Self {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
            vk: 0,
        };
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => hotkey.ctrl = true,
                "alt" => hotkey.alt = true,
                "shift" => hotkey.shift = true,
                "win" | "super" | "meta" => hotkey.win = true,
                name => {
                    let vk =
                        vk_for(name).ok_or_else(|| HotkeyError::UnknownKey(part.to_owned()))?;
                    if key.replace(vk).is_some() {
                        return Err(HotkeyError::TwoKeys);
                    }
                }
            }
        }
        hotkey.vk = key.ok_or(HotkeyError::NoKey)?;
        if !(hotkey.ctrl || hotkey.alt || hotkey.shift || hotkey.win) {
            return Err(HotkeyError::NoModifier);
        }
        Ok(hotkey)
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
            (self.win, "Win+"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        match key_name(self.vk) {
            Some(name) => f.write_str(&name),
            None => write!(f, "0x{:02X}", self.vk),
        }
    }
}

/// Named keys and their virtual-key codes.
const NAMED: [(&str, u32); 15] = [
    ("space", 0x20),
    ("enter", 0x0D),
    ("tab", 0x09),
    ("esc", 0x1B),
    ("backspace", 0x08),
    ("insert", 0x2D),
    ("delete", 0x2E),
    ("home", 0x24),
    ("end", 0x23),
    ("pageup", 0x21),
    ("pagedown", 0x22),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
];

/// The virtual-key code for a lower-case key name.
fn vk_for(name: &str) -> Option<u32> {
    if let Some(&(_, vk)) = NAMED.iter().find(|(n, _)| *n == name) {
        return Some(vk);
    }
    match name {
        "escape" => return Some(0x1B),
        "return" => return Some(0x0D),
        _ => {}
    }
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        // A-Z and 0-9 map to their upper-case ASCII codes.
        if c.is_ascii_alphanumeric() {
            return Some(u32::from(c.to_ascii_uppercase()));
        }
    }
    let n: u32 = name.strip_prefix('f')?.parse().ok()?;
    (1..=24).contains(&n).then(|| 0x70 + n - 1)
}

/// The display name for a virtual-key code [`vk_for`] produces.
fn key_name(vk: u32) -> Option<String> {
    if let Some(&(name, _)) = NAMED.iter().find(|(_, v)| *v == vk) {
        let mut name = name.to_owned();
        if let Some(first) = name.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        return Some(match name.as_str() {
            "Pageup" => "PageUp".to_owned(),
            "Pagedown" => "PageDown".to_owned(),
            _ => name,
        });
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(String::from),
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_defaults_parse() {
        let voice = Hotkey::parse("Ctrl+Alt+Space").unwrap();
        assert!(voice.ctrl && voice.alt && !voice.shift && !voice.win);
        assert_eq!(voice.vk, 0x20);
        assert_eq!(Hotkey::parse("Ctrl+Alt+T").unwrap().vk, u32::from(b'T'));
    }

    #[test]
    fn parsing_is_forgiving_about_case_and_spaces() {
        assert_eq!(
            Hotkey::parse(" ctrl + SHIFT + f12 ").unwrap(),
            Hotkey {
                ctrl: true,
                alt: false,
                shift: true,
                win: false,
                vk: 0x7B,
            }
        );
        assert_eq!(Hotkey::parse("Win+Alt+7").unwrap().vk, u32::from(b'7'));
        assert_eq!(Hotkey::parse("Alt+Down").unwrap().vk, 0x28);
    }

    #[test]
    fn bad_strings_are_rejected() {
        assert_eq!(Hotkey::parse(""), Err(HotkeyError::Empty));
        assert_eq!(Hotkey::parse("Space"), Err(HotkeyError::NoModifier));
        assert_eq!(Hotkey::parse("Ctrl+Alt"), Err(HotkeyError::NoKey));
        assert_eq!(Hotkey::parse("Ctrl+A+B"), Err(HotkeyError::TwoKeys));
        assert_eq!(
            Hotkey::parse("Ctrl+F25"),
            Err(HotkeyError::UnknownKey("F25".to_owned()))
        );
        assert_eq!(
            Hotkey::parse("Ctrl+Hyper"),
            Err(HotkeyError::UnknownKey("Hyper".to_owned()))
        );
        assert_eq!(
            Hotkey::parse("Ctrl++"),
            Err(HotkeyError::UnknownKey(String::new()))
        );
    }

    #[test]
    fn display_round_trips() {
        for text in [
            "Ctrl+Alt+Space",
            "Ctrl+Alt+T",
            "Shift+Win+F7",
            "Alt+PageDown",
            "Ctrl+Down",
            "Ctrl+0",
        ] {
            let parsed = Hotkey::parse(text).unwrap();
            assert_eq!(parsed.to_string(), text);
            assert_eq!(Hotkey::parse(&parsed.to_string()), Ok(parsed));
        }
    }
}
