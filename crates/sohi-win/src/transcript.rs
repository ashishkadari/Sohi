//! Full mode's conversation (spec §4.9): a read-only `RichEdit`.
//!
//! Text only ever goes in as plain text with a format set beforehand, so
//! nothing in it can change how the control looks.

use windows::Win32::Foundation::{FreeLibrary, HMODULE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::Win32::UI::Controls::RichEdit::{
    CFE_BOLD, CFE_EFFECTS, CFM_BACKCOLOR, CFM_BOLD, CFM_COLOR, CFM_FACE, CFM_SIZE, CHARFORMAT2W,
    CHARRANGE, EM_EXGETSEL, EM_EXLIMITTEXT, EM_EXSETSEL, EM_SETBKGNDCOLOR, EM_SETCHARFORMAT,
    EM_SETEDITSTYLE, EM_SETPARAFORMAT, PARAFORMAT, PFM_RIGHTINDENT, PFM_STARTINDENT, SCF_SELECTION,
    SES_EXTENDBACKCOLOR,
};
use windows::Win32::UI::Controls::{EM_REPLACESEL, SetWindowTheme};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, ES_MULTILINE, ES_NOHIDESEL, ES_READONLY, GetScrollInfo, HMENU, SB_BOTTOM,
    SB_VERT, SCROLLINFO, SIF_ALL, SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_SETREDRAW,
    WM_VSCROLL, WS_CHILD, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::WinError;
use crate::gdi::rgb;
use crate::hwnd::module_instance;

/// How a piece of the conversation looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextStyle {
    /// What the user said or typed.
    User,
    /// Ordinary text of a reply.
    Body,
    /// Bold text of a reply.
    Bold,
    /// A heading.
    Heading,
    /// Code within a line.
    InlineCode,
    /// A block of code.
    Code,
    /// A small grey note, such as a code block's language.
    Label,
}

/// Font face, size in twips, weight, colours, and indent in twips.
struct Look {
    face: PCWSTR,
    size: i32,
    bold: bool,
    text: u32,
    ground: u32,
    indent: i32,
}

/// The window's ground.
pub(crate) const GROUND: u32 = 0x0E_0A_0C;
const CODE_GROUND: u32 = 0x1F_15_19;
const UI: PCWSTR = w!("Segoe UI Variable Text");
const MONO: PCWSTR = w!("Cascadia Mono");
const CODE_INDENT: i32 = 200;

impl TextStyle {
    fn look(self) -> Look {
        let body = Look {
            face: UI,
            size: 240,
            bold: false,
            text: 0xED_ED_ED,
            ground: GROUND,
            indent: 0,
        };
        match self {
            Self::Body => body,
            Self::User => Look {
                text: 0xFF_4D_5E,
                bold: true,
                ..body
            },
            Self::Bold => Look { bold: true, ..body },
            Self::Heading => Look {
                bold: true,
                size: 290,
                ..body
            },
            Self::InlineCode => Look {
                face: MONO,
                size: 220,
                text: 0xF0_C4_CA,
                ground: CODE_GROUND,
                ..body
            },
            Self::Code => Look {
                face: MONO,
                size: 220,
                text: 0xE6_D8_DB,
                ground: CODE_GROUND,
                indent: CODE_INDENT,
                ..body
            },
            Self::Label => Look {
                size: 180,
                text: 0x8A_84_86,
                indent: CODE_INDENT,
                ..body
            },
        }
    }
}

/// The library with the control; unloaded on drop.
struct RichEdit(HMODULE);

impl RichEdit {
    fn load() -> Result<Self, WinError> {
        // SAFETY: loads a system DLL from System32 only.
        unsafe { LoadLibraryExW(w!("msftedit.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
            .map(Self)
            .map_err(|e| WinError::api("LoadLibraryExW(msftedit)", &e))
    }
}

impl Drop for RichEdit {
    fn drop(&mut self) {
        // SAFETY: our own reference; `Transcript`'s owner destroys the
        // control first (see `FullWindow`'s field order).
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

/// The control. Destroyed with its parent window, which must go before
/// this does.
pub(crate) struct Transcript {
    hwnd: HWND,
    _library: RichEdit,
}

impl Transcript {
    /// Creates the control in `parent`.
    pub(crate) fn create(parent: HWND) -> Result<Self, WinError> {
        let library = RichEdit::load()?;
        let bits = ES_MULTILINE | ES_READONLY | ES_NOHIDESEL;
        let style =
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_TABSTOP | WINDOW_STYLE(bits.cast_unsigned());
        // SAFETY: `parent` is a live window on this thread; the class comes
        // with the library just loaded.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("RICHEDIT50W"),
                PCWSTR::null(),
                style,
                0,
                0,
                0,
                0,
                Some(parent),
                Some(HMENU::default()),
                Some(module_instance()?),
                None,
            )
        }
        .map_err(|e| WinError::api("CreateWindowExW(RichEdit)", &e))?;
        let this = Self {
            hwnd,
            _library: library,
        };
        let ground = isize::try_from(rgb(GROUND).0).unwrap_or(0);
        this.send(EM_SETBKGNDCOLOR, 0, ground);
        this.send(EM_EXLIMITTEXT, 0, 8_000_000);
        // Code has its ground out to the edge.
        let extend = SES_EXTENDBACKCOLOR as usize;
        this.send(EM_SETEDITSTYLE, extend, extend.cast_signed());
        // SAFETY: our control; a missing theme leaves light scroll bars.
        let _ = unsafe { SetWindowTheme(hwnd, w!("DarkMode_Explorer"), PCWSTR::null()) };
        Ok(this)
    }

    /// The control, for placing and subclassing.
    pub(crate) fn raw(&self) -> HWND {
        self.hwnd
    }

    /// Adds `text` at the end. What the user has selected stays so, and
    /// the end stays in view if it was.
    pub(crate) fn append(&self, text: &str, style: TextStyle) {
        let text = clean(text);
        if text.is_empty() {
            return;
        }
        let follow = self.at_end();
        let selected = self.selection();
        self.send(WM_SETREDRAW, 0, 0);
        self.select(CHARRANGE {
            cpMin: i32::MAX,
            cpMax: i32::MAX,
        });
        self.set_format(&style.look());
        for piece in pieces(&text) {
            let piece = HSTRING::from(piece);
            self.send(EM_REPLACESEL, 0, piece.as_ptr() as isize);
        }
        if selected.cpMin != selected.cpMax {
            self.select(selected);
        }
        self.send(WM_SETREDRAW, 1, 0);
        if follow {
            self.send(WM_VSCROLL, usize::try_from(SB_BOTTOM.0).unwrap_or(7), 0);
        }
        // SAFETY: our control.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    /// Removes everything.
    pub(crate) fn clear(&self) {
        self.select(CHARRANGE {
            cpMin: 0,
            cpMax: -1,
        });
        let empty = HSTRING::new();
        self.send(EM_REPLACESEL, 0, empty.as_ptr() as isize);
    }

    fn set_format(&self, look: &Look) {
        let mut format = CHARFORMAT2W::default();
        format.Base.cbSize = u32::try_from(size_of::<CHARFORMAT2W>()).unwrap_or(0);
        format.Base.dwMask = CFM_FACE | CFM_SIZE | CFM_BOLD | CFM_COLOR | CFM_BACKCOLOR;
        format.Base.dwEffects = if look.bold { CFE_BOLD } else { CFE_EFFECTS(0) };
        format.Base.yHeight = look.size;
        format.Base.crTextColor = rgb(look.text);
        format.crBackColor = rgb(look.ground);
        // SAFETY: both faces are static NUL-terminated strings.
        let face = unsafe { look.face.as_wide() };
        let mut name = [0u16; 32];
        for (to, from) in name.iter_mut().zip(face.iter().take(31)) {
            *to = *from;
        }
        format.Base.szFaceName = name;
        self.send(
            EM_SETCHARFORMAT,
            SCF_SELECTION as usize,
            (&raw const format) as isize,
        );
        let para = PARAFORMAT {
            cbSize: u32::try_from(size_of::<PARAFORMAT>()).unwrap_or(0),
            dwMask: PFM_STARTINDENT | PFM_RIGHTINDENT,
            dxStartIndent: look.indent,
            dxRightIndent: CODE_INDENT,
            ..PARAFORMAT::default()
        };
        self.send(EM_SETPARAFORMAT, 0, (&raw const para) as isize);
    }

    /// Whether the end of the text is in view.
    fn at_end(&self) -> bool {
        let mut info = SCROLLINFO {
            cbSize: u32::try_from(size_of::<SCROLLINFO>()).unwrap_or(0),
            fMask: SIF_ALL,
            ..SCROLLINFO::default()
        };
        // SAFETY: our control, and a valid SCROLLINFO with its size set.
        if unsafe { GetScrollInfo(self.hwnd, SB_VERT, &raw mut info) }.is_err() {
            return true;
        }
        let page = i32::try_from(info.nPage).unwrap_or(0);
        info.nPos + page + 40 >= info.nMax
    }

    fn selection(&self) -> CHARRANGE {
        let mut range = CHARRANGE::default();
        self.send(EM_EXGETSEL, 0, (&raw mut range) as isize);
        range
    }

    fn select(&self, range: CHARRANGE) {
        self.send(EM_EXSETSEL, 0, (&raw const range) as isize);
    }

    /// Sends one of the messages above to the control.
    fn send(&self, msg: u32, wparam: usize, lparam: isize) -> isize {
        // SAFETY: our control, on this thread. Every pointer passed refers
        // to a live local of the type the message takes, and the control
        // keeps none of them.
        unsafe { SendMessageW(self.hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
    }
}

/// Splits `text` so that no piece begins like rich text (`{\rtf`), which
/// the control would interpret even here: braces at the start go alone.
fn pieces(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let cut = if rest.starts_with('{') { 1 } else { rest.len() };
        let (piece, after) = rest.split_at(cut);
        rest = after;
        (!piece.is_empty()).then_some(piece)
    })
}

/// Text as the control takes it: paragraphs end in `\r`, and no other
/// control characters.
fn clean(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' => Some('\r'),
            '\t' => Some('\t'),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hwnd::{OwnedHwnd, WindowSpec, WndHandler, create_window};
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW, WS_POPUP};

    struct Plain;
    impl WndHandler for Plain {
        fn on_message(&self, _: HWND, _: u32, _: WPARAM, _: LPARAM) -> Option<LRESULT> {
            None
        }
    }

    fn fixture() -> (OwnedHwnd, Transcript) {
        let spec = WindowSpec {
            class_name: "Sohi.TestTranscript",
            ex_style: WINDOW_EX_STYLE(0),
            style: WS_POPUP,
        };
        let parent = create_window(&spec, Box::new(Plain)).unwrap();
        let transcript = Transcript::create(parent.raw()).unwrap();
        (parent, transcript)
    }

    fn text(t: &Transcript) -> String {
        // SAFETY: our control; the buffer has room for the terminator.
        unsafe {
            let len = usize::try_from(GetWindowTextLengthW(t.hwnd)).unwrap();
            let mut buffer = vec![0u16; len + 1];
            let n = usize::try_from(GetWindowTextW(t.hwnd, &mut buffer)).unwrap();
            String::from_utf16_lossy(&buffer[..n]).replace("\r\n", "\n")
        }
    }

    #[test]
    fn text_is_added_at_the_end_and_cleared() {
        let (parent, t) = fixture();
        t.append("you\n", TextStyle::User);
        t.append("fn main() {\n\tx\n}", TextStyle::Code);
        assert_eq!(text(&t), "you\nfn main() {\n\tx\n}");
        t.clear();
        assert_eq!(text(&t), "");
        drop(parent);
    }

    #[test]
    fn rich_text_is_not_interpreted() {
        let (parent, t) = fixture();
        let mut all = String::new();
        for rtf in [
            "{\\rtf1\\ansi{\\b bold}}",
            "{{\\rtf1 x}}",
            " {\\rtf1 x}",
            "\n{\\rtf1 x}",
            "{\\urtf1 x}",
            "{",
        ] {
            t.append(rtf, TextStyle::Body);
            all.push_str(rtf);
            assert_eq!(text(&t), all);
        }
        t.append("a\0b\x07c", TextStyle::Body);
        assert_eq!(text(&t), format!("{all}abc"));
        drop(parent);
    }

    #[test]
    fn the_users_selection_survives_new_text() {
        let (parent, t) = fixture();
        t.append("hello world", TextStyle::Body);
        t.select(CHARRANGE { cpMin: 0, cpMax: 5 });
        t.append(" more", TextStyle::Body);
        let range = t.selection();
        assert_eq!((range.cpMin, range.cpMax), (0, 5));
        assert_eq!(text(&t), "hello world more");
        drop(parent);
    }
}
