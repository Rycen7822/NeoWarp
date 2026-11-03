use pathfinder_geometry::rect::RectF;
use windows::Win32::Foundation::{E_FAIL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::Shell::{
    DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass,
};
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

/// Bounds are physical client pixels. Only the HWND's UI thread updates or reads this state.
pub fn set_maximize_button_rect(
    window: &WinitWindow,
    bounds: Option<RectF>,
) -> Result<(), WindowAttributeErr> {
    let hwnd = to_hwnd(window)?;
    let rect = bounds.map(|bounds| RECT {
        left: bounds.min_x().ceil() as i32,
        top: bounds.min_y().ceil() as i32,
        right: bounds.max_x().floor() as i32,
        bottom: bounds.max_y().floor() as i32,
    });
    let mut state = 0;
    // SAFETY: winit renders and dispatches window messages on the creating UI thread. The
    // subclass owns its boxed RECT until WM_NCDESTROY; no reference is kept across message calls.
    unsafe {
        if GetWindowSubclass(
            hwnd,
            Some(maximize_button_proc),
            MAXIMIZE_BUTTON_SUBCLASS,
            Some(&mut state),
        )
        .as_bool()
        {
            *(state as *mut RECT) = rect.unwrap_or_default();
        } else if let Some(rect) = rect {
            let state = Box::into_raw(Box::new(rect));
            if !SetWindowSubclass(
                hwnd,
                Some(maximize_button_proc),
                MAXIMIZE_BUTTON_SUBCLASS,
                state as usize,
            )
            .as_bool()
            {
                drop(Box::from_raw(state));
                return Err(windows::core::Error::new(E_FAIL, "SetWindowSubclass failed").into());
            }
        }
    }
    Ok(())
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
    // SAFETY: SetWindowSubclass owns a live RECT for this callback, released on WM_NCDESTROY.
    // Copy it before forwarding messages: forwarding may re-enter this callback or destroy hwnd.
    let rect = unsafe { *(state as *const RECT) };
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
            let _ =
                RemoveWindowSubclass(hwnd, Some(maximize_button_proc), MAXIMIZE_BUTTON_SUBCLASS);
            drop(Box::from_raw(state as *mut RECT));
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
