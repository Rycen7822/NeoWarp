//! The first-party native plugin boundary. All pointers are borrowed for the call;
//! callbacks retain/release their host context, and each side frees its own allocations.

use std::ffi::c_void;

pub const ABI_VERSION: u32 = 1;
pub const ACCESSIBILITY: u32 = 1;
pub const CLICK: u32 = 1;
pub const EXPAND: u32 = 2;
pub const COLLAPSE: u32 = 3;
pub const SELECTED: u32 = 1;
pub const CHECKABLE: u32 = 2;
pub const CHECKED: u32 = 4;
pub const EXPANDABLE: u32 = 8;
pub const EXPANDED: u32 = 16;

#[repr(C)]
pub struct PluginHeader {
    pub abi_version: u32,
    pub capability: u32,
    pub size: usize,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct HostCallbacks {
    pub context: usize,
    pub retain: unsafe extern "C" fn(usize),
    pub release: unsafe extern "C" fn(usize),
    /// Target zero requests a fresh semantic frame; other targets invoke a current control.
    pub dispatch: unsafe extern "C" fn(usize, u64, u32),
}

#[repr(C)]
pub struct SemanticNode {
    pub id: u64,
    pub label: *const u8,
    pub label_len: usize,
    pub bounds: [f64; 4],
    pub flags: u32,
}

#[repr(C)]
pub struct AccessibilityApi {
    pub header: PluginHeader,
    pub create: unsafe extern "C" fn(isize, HostCallbacks) -> *mut c_void,
    pub is_active: unsafe extern "C" fn(*mut c_void) -> bool,
    pub update: unsafe extern "C" fn(*mut c_void, *const SemanticNode, usize) -> bool,
    pub destroy: unsafe extern "C" fn(*mut c_void),
}

pub type PluginEntry = unsafe extern "C" fn() -> *const PluginHeader;
