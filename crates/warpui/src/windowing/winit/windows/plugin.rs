use std::ffi::c_void;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, ensure};
use neowarp_plugin_api as api;
use serde::Deserialize;
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    LoadLibraryExW,
};
use windows::core::{HSTRING, s};
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::WindowId;
use crate::windowing::winit::CustomEvent;

#[derive(Deserialize)]
struct Manifest {
    id: String,
    abi_version: u32,
    library: String,
}

struct Module(HMODULE);

impl Drop for Module {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

fn library() -> Result<&'static api::AccessibilityApi, &'static str> {
    // Loaded only by an enabled window. Keep code alive for COM references until process exit.
    static LIBRARY: OnceLock<Result<(usize, &'static api::AccessibilityApi), String>> =
        OnceLock::new();
    LIBRARY
        .get_or_init(|| load_library().map_err(|error| format!("{error:#}")))
        .as_ref()
        .map(|(_, api)| *api)
        .map_err(String::as_str)
}

fn load_library() -> Result<(usize, &'static api::AccessibilityApi)> {
    let exe = std::env::current_exe()?;
    let directory = exe
        .parent()
        .context("No executable directory")?
        .join("plugins/accessibility.windows");
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(directory.join("plugin.json"))?)?;
    ensure!(
        manifest.id == "accessibility.windows" && manifest.abi_version == api::ABI_VERSION,
        "Incompatible plugin manifest"
    );
    ensure!(
        std::path::Path::new(&manifest.library).components().count() == 1
            && matches!(
                std::path::Path::new(&manifest.library).components().next(),
                Some(std::path::Component::Normal(_))
            ),
        "Plugin library must be a filename"
    );
    let path = HSTRING::from(directory.join(manifest.library).as_os_str());
    let module = Module(unsafe {
        LoadLibraryExW(
            &path,
            None,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )?
    });
    let entry = unsafe { GetProcAddress(module.0, s!("neowarp_plugin_v1")) }
        .context("Plugin entry is missing")?;
    let entry: api::PluginEntry = unsafe { std::mem::transmute(entry) };
    let header = unsafe { entry().as_ref() }.context("Plugin entry returned null")?;
    ensure!(
        header.abi_version == api::ABI_VERSION
            && header.capability == api::ACCESSIBILITY
            && header.size == std::mem::size_of::<api::AccessibilityApi>(),
        "Incompatible plugin API"
    );
    let api = unsafe { &*(header as *const api::PluginHeader).cast::<api::AccessibilityApi>() };
    let handle = module.0.0 as usize;
    std::mem::forget(module);
    Ok((handle, api))
}

struct Bridge {
    window: WindowId,
    proxy: EventLoopProxy<CustomEvent>,
}

unsafe extern "C" fn retain(context: usize) {
    unsafe { Arc::increment_strong_count(context as *const Bridge) };
}

unsafe extern "C" fn release(context: usize) {
    unsafe { Arc::decrement_strong_count(context as *const Bridge) };
}

unsafe extern "C" fn dispatch(context: usize, target: u64, action: u32) {
    let bridge = unsafe { &*(context as *const Bridge) };
    let window = bridge.window;
    if bridge
        .proxy
        .send_event(CustomEvent::UpdateUIApp(Box::new(move |ctx| {
            ctx.invoke_accessibility(window, target, action);
        })))
        .is_err()
    {
        log::debug!("Ignoring accessibility request after the event loop stopped");
    }
}

pub(crate) struct WindowPlugin {
    api: &'static api::AccessibilityApi,
    instance: *mut c_void,
}

impl WindowPlugin {
    pub fn new(
        window: &winit::window::Window,
        id: WindowId,
        proxy: EventLoopProxy<CustomEvent>,
    ) -> Result<Self> {
        let api = library().map_err(anyhow::Error::msg)?;
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            anyhow::bail!("Accessibility requires a Win32 window");
        };
        let bridge = Arc::new(Bridge { window: id, proxy });
        let callbacks = api::HostCallbacks {
            context: Arc::as_ptr(&bridge) as usize,
            retain,
            release,
            dispatch,
        };
        let instance = unsafe { (api.create)(handle.hwnd.get(), callbacks) };
        ensure!(
            !instance.is_null(),
            "Accessibility plugin could not attach to the window"
        );
        Ok(Self { api, instance })
    }

    pub fn is_active(&self) -> bool {
        unsafe { (self.api.is_active)(self.instance) }
    }

    pub fn update(&mut self, nodes: &[api::SemanticNode]) {
        if !unsafe { (self.api.update)(self.instance, nodes.as_ptr(), nodes.len()) } {
            log::warn!("Accessibility plugin rejected a semantic frame");
        }
    }
}

impl Drop for WindowPlugin {
    fn drop(&mut self) {
        unsafe { (self.api.destroy)(self.instance) };
    }
}
