//! The full mode window's procedure, its painting, and its keyboard.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sohi_core::full::{BUTTONS, Layout};
use sohi_core::geometry::RectPx;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DrawTextW, EndPaint, FillRect, FrameRect, HBRUSH, HDC, PAINTSTRUCT,
    SRCCOPY, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_RETURN, VK_SHIFT,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WM_CHAR, WM_CLOSE, WM_COMMAND, WM_CTLCOLOREDIT, WM_DRAWITEM, WM_ERASEBKGND,
    WM_KEYDOWN, WM_NCDESTROY, WM_PAINT, WM_SETFOCUS,
};

use crate::gdi::{Brush, Font, rgb};
use crate::hwnd::WndHandler;
use crate::surface::Surface;
use crate::transcript::GROUND;
use crate::{EventQueue, FullEvent, WinEvent};

const TEXT: u32 = 0xED_ED_ED;
const DIM: u32 = 0xA8_9A_9D;
const PANEL: u32 = 0x19_11_14;
const PRESSED: u32 = 0x5A_0F_1A;
const EDGE: u32 = 0x6D_0B_1A;
const LINE: u32 = 0x2E_1C_21;

/// The buttons, in the layout's order.
pub(crate) const BUTTON_EVENTS: [FullEvent; BUTTONS] = [
    FullEvent::Talk,
    FullEvent::CopyCode,
    FullEvent::Reminders,
    FullEvent::NewChat,
    FullEvent::Exit,
];

/// Everything painting needs, shared with the window's procedure.
pub(crate) struct Paint {
    pub(crate) layout: Cell<Layout>,
    pub(crate) size: Cell<(i32, i32)>,
    pub(crate) fonts: RefCell<Option<Fonts>>,
    pub(crate) status: RefCell<String>,
    pub(crate) labels: RefCell<[&'static str; BUTTONS]>,
    pub(crate) avatar: RefCell<Option<Surface>>,
    brushes: Brushes,
}

/// The window's fonts at one DPI.
pub(crate) struct Fonts {
    pub(crate) input: Font,
    pub(crate) small: Font,
    pub(crate) title: Font,
}

struct Brushes {
    ground: Brush,
    panel: Brush,
    pressed: Brush,
    edge: Brush,
    line: Brush,
}

impl Paint {
    pub(crate) fn new() -> Result<Self, crate::WinError> {
        let solid = |c| Brush::solid(rgb(c));
        Ok(Self {
            layout: Cell::default(),
            size: Cell::default(),
            fonts: RefCell::new(None),
            status: RefCell::default(),
            labels: RefCell::new(["Talk", "Copy code", "Reminders", "New chat", "Exit"]),
            avatar: RefCell::new(None),
            brushes: Brushes {
                ground: solid(GROUND)?,
                panel: solid(PANEL)?,
                pressed: solid(PRESSED)?,
                edge: solid(EDGE)?,
                line: solid(LINE)?,
            },
        })
    }

    fn fill(dc: HDC, r: RectPx, brush: HBRUSH) {
        let r = rect(r);
        // SAFETY: a valid DC and a brush that outlives the call.
        unsafe { FillRect(dc, &raw const r, brush) };
    }

    /// Draws one line of text.
    fn text(dc: HDC, r: RectPx, text: &str, font: &Font, color: u32, format: DRAW_TEXT_FORMAT) {
        let mut r = rect(r);
        let mut text: Vec<u16> = text.encode_utf16().collect();
        if text.is_empty() {
            return;
        }
        // SAFETY: a valid DC; the font is deselected again; DrawTextW gets
        // no DT_MODIFYSTRING.
        unsafe {
            let old = font.select(dc);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, rgb(color));
            DrawTextW(
                dc,
                &mut text,
                &raw mut r,
                format | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            Font::restore(dc, old);
        }
    }

    fn paint(&self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: paired with EndPaint below.
        let dc = unsafe { BeginPaint(hwnd, &raw mut ps) };
        let l = self.layout.get();
        let (width, height) = self.size.get();
        let all = RectPx {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        Self::fill(dc, all, self.brushes.ground.raw());
        Self::fill(dc, l.separator, self.brushes.line.raw());
        Self::fill(dc, l.input_frame, self.brushes.panel.raw());
        let frame = rect(l.input_frame);
        // SAFETY: a valid DC and brush.
        unsafe { FrameRect(dc, &raw const frame, self.brushes.edge.raw()) };
        self.draw_avatar(dc);
        if let Some(fonts) = self.fonts.borrow().as_ref() {
            Self::text(dc, l.title, "SOHI", &fonts.title, TEXT, DT_LEFT);
            let status = self.status.borrow();
            Self::text(dc, l.status, &status, &fonts.small, DIM, DT_LEFT);
        }
        // SAFETY: the DC from BeginPaint.
        let _ = unsafe { EndPaint(hwnd, &raw const ps) };
    }

    /// Copies the head into the header.
    pub(crate) fn draw_avatar(&self, dc: HDC) {
        let at = self.layout.get().avatar;
        if let Some(surface) = self.avatar.borrow().as_ref() {
            let (w, h) = surface.size();
            // SAFETY: both DCs are valid, and the surface has that size.
            let _ = unsafe {
                BitBlt(
                    dc,
                    at.left,
                    at.top,
                    w.cast_signed(),
                    h.cast_signed(),
                    Some(surface.dc()),
                    0,
                    0,
                    SRCCOPY,
                )
            };
        }
    }

    fn draw_button(&self, item: &DRAWITEMSTRUCT) {
        let index = usize::try_from(item.CtlID)
            .ok()
            .and_then(|id| id.checked_sub(FIRST_BUTTON));
        let labels = self.labels.borrow();
        let Some(label) = index.and_then(|i| labels.get(i)) else {
            return;
        };
        let r = RectPx {
            left: item.rcItem.left,
            top: item.rcItem.top,
            right: item.rcItem.right,
            bottom: item.rcItem.bottom,
        };
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let focused = item.itemState.0 & ODS_FOCUS.0 != 0;
        let b = &self.brushes;
        let ground = if pressed { &b.pressed } else { &b.panel };
        Self::fill(item.hDC, r, ground.raw());
        let edge = if focused { &b.edge } else { &b.line };
        // SAFETY: the DC is the button's, valid during WM_DRAWITEM.
        unsafe { FrameRect(item.hDC, &raw const item.rcItem, edge.raw()) };
        if let Some(fonts) = self.fonts.borrow().as_ref() {
            Self::text(
                item.hDC,
                r,
                label,
                &fonts.small,
                TEXT,
                DT_CENTER | DT_VCENTER,
            );
        }
    }
}

/// The window's own messages.
pub(crate) struct FullHandler {
    pub(crate) paint: Rc<Paint>,
    pub(crate) queue: EventQueue,
    pub(crate) input: Rc<Cell<HWND>>,
}

/// Control id of the first button.
pub(crate) const FIRST_BUTTON: usize = 10;

impl WndHandler for FullHandler {
    fn on_message(&self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_PAINT => {
                self.paint.paint(hwnd);
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)), // WM_PAINT fills everything
            WM_CTLCOLOREDIT => {
                let dc = HDC(wparam.0 as *mut _);
                // SAFETY: `dc` is the control's, valid during this message.
                unsafe {
                    SetTextColor(dc, rgb(TEXT));
                    SetBkColor(dc, rgb(PANEL));
                }
                Some(LRESULT(self.paint.brushes.panel.raw().0 as isize))
            }
            WM_DRAWITEM => {
                // SAFETY: for WM_DRAWITEM, lparam points to a
                // DRAWITEMSTRUCT for the length of the message.
                let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
                self.paint.draw_button(item);
                Some(LRESULT(1))
            }
            WM_COMMAND => {
                let id = wparam.0 & 0xFFFF;
                let event = id
                    .checked_sub(FIRST_BUTTON)
                    .and_then(|i| BUTTON_EVENTS.get(i))?;
                // Only BN_CLICKED (0) counts.
                if wparam.0 >> 16 == 0 {
                    self.queue.push(WinEvent::Full(*event));
                    self.focus_input();
                }
                Some(LRESULT(0))
            }
            WM_SETFOCUS => {
                self.focus_input();
                Some(LRESULT(0))
            }
            WM_CLOSE => {
                // Alt+F4 means "leave", not "destroy": the app decides.
                self.queue.push(WinEvent::Full(FullEvent::Exit));
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

impl FullHandler {
    fn focus_input(&self) {
        let input = self.input.get();
        if !input.is_invalid() {
            // SAFETY: our child.
            let _ = unsafe { SetFocus(Some(input)) };
        }
    }
}

pub(crate) fn rect(r: RectPx) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// Subclass ids, which also tell the two controls apart.
pub(crate) const INPUT_ID: usize = 1;
pub(crate) const TRANSCRIPT_ID: usize = 2;

/// What the subclassed controls share.
pub(crate) struct Keys {
    pub(crate) queue: EventQueue,
    pub(crate) input: Rc<Cell<HWND>>,
}

fn down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: no preconditions.
    unsafe { GetKeyState(i32::from(key.0)) < 0 }
}

/// What a key press in one of the controls asks for.
fn shortcut(key: u16, input: bool) -> Option<FullEvent> {
    let (ctrl, shift) = (down(VK_CONTROL), down(VK_SHIFT));
    match key {
        k if k == VK_ESCAPE.0 => Some(FullEvent::Escape),
        k if k == VK_RETURN.0 && input && !shift && !ctrl => Some(FullEvent::Submit),
        0x4E if ctrl && !shift => Some(FullEvent::NewChat), // N
        0x52 if ctrl && !shift => Some(FullEvent::Reminders), // R
        0x4D if ctrl && !shift => Some(FullEvent::Talk),    // M
        0x43 if ctrl && shift => Some(FullEvent::CopyCode), // C
        _ => None,
    }
}

/// Subclass procedure for the input and the transcript.
pub(crate) unsafe extern "system" fn control_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: `data` is the `Keys` box set in `FullWindow::create`, alive
    // until after WM_NCDESTROY.
    let keys = unsafe { &*(data as *const Keys) };
    let key = u16::try_from(wparam.0).unwrap_or(0);
    match msg {
        WM_KEYDOWN => {
            if let Some(event) = shortcut(key, id == INPUT_ID) {
                keys.queue.push(WinEvent::Full(event));
                return LRESULT(0);
            }
        }
        // The characters of the shortcuts: Ctrl+Shift+C, Enter and Ctrl+M,
        // Ctrl+N, Esc. Shift+Enter breaks the line, so it stays.
        WM_CHAR if key == 0x0E || key == 0x1B => return LRESULT(0),
        WM_CHAR if key == 0x0D && (down(VK_CONTROL) || !down(VK_SHIFT)) => return LRESULT(0),
        WM_CHAR if key == 0x03 && down(VK_SHIFT) => return LRESULT(0),
        // Typing while the conversation has the focus goes to the input.
        WM_CHAR if id == TRANSCRIPT_ID && key >= 0x20 => {
            let input = keys.input.get();
            // SAFETY: our own child window.
            unsafe {
                let _ = SetFocus(Some(input));
                let _ = PostMessageW(Some(input), WM_CHAR, wparam, lparam);
            }
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            // SAFETY: removing our own subclass from our own window.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(control_proc), id) };
        }
        _ => {}
    }
    // SAFETY: forwarding unmodified arguments.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
