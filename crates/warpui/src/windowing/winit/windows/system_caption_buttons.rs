use std::cell::Cell;

use pathfinder_geometry::rect::RectF;
use windows::Win32::Foundation::{E_FAIL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    HTCLIENT, HTMAXBUTTON, SendMessageW, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
    WM_NCMOUSELEAVE, WM_NCMOUSEMOVE,
};
use winit::window::Window as WinitWindow;

use super::WindowAttributeErr;
use super::window_attribute::to_hwnd;

const MAXIMIZE_BUTTON_SUBCLASS: usize = 1;
const WM_MOUSELEAVE: u32 = 0x02a3;
const MK_LBUTTON: usize = 1;

/// The window owns its hook and state. Only its creating UI thread reads or updates the Cells.
pub struct MaximizeButton {
    state: Box<MaximizeButtonState>,
}

struct MaximizeButtonState {
    window: Cell<HWND>,
    bounds: Cell<RECT>,
}

fn physical_rect(bounds: RectF) -> RECT {
    RECT {
        left: bounds.min_x().ceil() as i32,
        top: bounds.min_y().ceil() as i32,
        right: bounds.max_x().floor() as i32,
        bottom: bounds.max_y().floor() as i32,
    }
}

impl MaximizeButton {
    pub fn new(window: &WinitWindow, bounds: RectF) -> Result<Self, WindowAttributeErr> {
        let hwnd = to_hwnd(window)?;
        let state = Box::new(MaximizeButtonState {
            window: Cell::new(hwnd),
            bounds: Cell::new(physical_rect(bounds)),
        });
        // SAFETY: Box keeps the callback address stable. Drop removes the hook before freeing
        // the state; WM_NCDESTROY clears the HWND to avoid removing a hook from a reused handle.
        if !unsafe {
            SetWindowSubclass(
                hwnd,
                Some(maximize_button_proc),
                MAXIMIZE_BUTTON_SUBCLASS,
                state.as_ref() as *const MaximizeButtonState as usize,
            )
        }
        .as_bool()
        {
            return Err(windows::core::Error::new(E_FAIL, "SetWindowSubclass failed").into());
        }
        Ok(Self { state })
    }

    pub fn set_bounds(&self, bounds: Option<RectF>) {
        self.state
            .bounds
            .set(bounds.map(physical_rect).unwrap_or_default());
    }
}

impl Drop for MaximizeButton {
    fn drop(&mut self) {
        let hwnd = self.state.window.get();
        if !hwnd.is_invalid() {
            // SAFETY: this hook belongs to this window and is removed on its UI thread.
            unsafe {
                let _ = RemoveWindowSubclass(
                    hwnd,
                    Some(maximize_button_proc),
                    MAXIMIZE_BUTTON_SUBCLASS,
                );
            }
        }
    }
}

/// Convert signed screen coordinates (including monitors left/above the primary) to client pixels.
fn client_point(hwnd: HWND, lparam: LPARAM) -> Option<POINT> {
    let mut point = POINT {
        x: lparam.0 as i16 as i32,
        y: (lparam.0 >> 16) as i16 as i32,
    };
    // SAFETY: hwnd belongs to the active subclass callback; point is writable for this call.
    unsafe { ScreenToClient(hwnd, &mut point) }
        .as_bool()
        .then_some(point)
}

fn point_lparam(point: POINT) -> LPARAM {
    LPARAM((point.x as u16 as u32 | ((point.y as u16 as u32) << 16)) as isize)
}

unsafe extern "system" fn maximize_button_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    state: usize,
) -> LRESULT {
    // SAFETY: MaximizeButton keeps its state alive while the hook is installed. Copy bounds
    // before forwarding: a forwarded message may re-enter the hook or drop the owning window.
    let rect = unsafe { (*(state as *const MaximizeButtonState)).bounds.get() };
    match message {
        WM_NCHITTEST => {
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            if result.0 == HTCLIENT as isize
                && client_point(hwnd, lparam).is_some_and(|point| {
                    point.x >= rect.left
                        && point.x < rect.right
                        && point.y >= rect.top
                        && point.y < rect.bottom
                })
            {
                return LRESULT(HTMAXBUTTON as isize);
            }
            return result;
        }
        WM_NCMOUSEMOVE if wparam.0 == HTMAXBUTTON as usize => {
            if let Some(point) = client_point(hwnd, lparam) {
                // Keep WarpUI's existing hover styling; DefSubclassProc below still receives
                // the non-client message, so Windows owns the Snap Layouts flyout.
                unsafe { SendMessageW(hwnd, WM_MOUSEMOVE, None, Some(point_lparam(point))) };
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    ..Default::default()
                };
                if let Err(error) = unsafe { TrackMouseEvent(&mut tracking) } {
                    log::debug!("Could not track caption hover exit: {error}");
                }
            }
        }
        WM_NCMOUSELEAVE => {
            unsafe { SendMessageW(hwnd, WM_MOUSELEAVE, None, None) };
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONUP | WM_NCLBUTTONDBLCLK
            if wparam.0 == HTMAXBUTTON as usize =>
        {
            if let Some(point) = client_point(hwnd, lparam) {
                let client_message = match message {
                    WM_NCLBUTTONUP => WM_LBUTTONUP,
                    WM_NCLBUTTONDBLCLK => WM_LBUTTONDBLCLK,
                    _ => WM_LBUTTONDOWN,
                };
                // Route one click through the existing Hoverable/capture/action path. Letting
                // DefWindowProc handle it too would maximize/restore the window twice.
                return unsafe {
                    SendMessageW(
                        hwnd,
                        client_message,
                        Some(WPARAM(if message == WM_NCLBUTTONUP {
                            0
                        } else {
                            MK_LBUTTON
                        })),
                        Some(point_lparam(point)),
                    )
                };
            }
        }
        WM_NCDESTROY => unsafe {
            (*(state as *const MaximizeButtonState))
                .window
                .set(HWND::default());
            let _ =
                RemoveWindowSubclass(hwnd, Some(maximize_button_proc), MAXIMIZE_BUTTON_SUBCLASS);
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
