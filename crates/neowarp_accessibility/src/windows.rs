use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use accesskit::{
    Action, ActionHandler, ActionRequest, ActivationHandler, Node, NodeId, Role, TreeId, TreeInfo,
    TreeUpdate,
};
use accesskit_windows::SubclassingAdapter;
use neowarp_plugin_api as api;
use windows::Win32::Foundation::HWND;

const ROOT: NodeId = NodeId(1);

struct Callbacks(api::HostCallbacks);

impl Callbacks {
    fn dispatch(&self, target: u64, action: u32) {
        // The host retains this context until the last provider callback is dropped.
        unsafe { (self.0.dispatch)(self.0.context, target, action) };
    }
}

impl Drop for Callbacks {
    fn drop(&mut self) {
        unsafe { (self.0.release)(self.0.context) };
    }
}

struct Activate {
    callbacks: Arc<Callbacks>,
    active: Arc<AtomicBool>,
}

impl ActivationHandler for Activate {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.active.store(true, Ordering::Release);
        self.callbacks.dispatch(0, 0);
        None
    }
}

struct Invoke(Arc<Callbacks>);

impl ActionHandler for Invoke {
    fn do_action(&mut self, request: ActionRequest) {
        let action = match request.action {
            Action::Click => api::CLICK,
            Action::Expand => api::EXPAND,
            Action::Collapse => api::COLLAPSE,
            _ => return,
        };
        self.0.dispatch(request.target_node.0, action);
    }
}

struct Instance {
    adapter: SubclassingAdapter,
    active: Arc<AtomicBool>,
}

unsafe extern "C" fn create(hwnd: isize, host: api::HostCallbacks) -> *mut c_void {
    catch_unwind(AssertUnwindSafe(|| {
        unsafe { (host.retain)(host.context) };
        let callbacks = Arc::new(Callbacks(host));
        let active = Arc::new(AtomicBool::new(false));
        let adapter = SubclassingAdapter::new(
            HWND(hwnd as *mut _),
            Activate {
                callbacks: callbacks.clone(),
                active: active.clone(),
            },
            Invoke(callbacks),
        );
        Box::into_raw(Box::new(Instance { adapter, active })).cast()
    }))
    .unwrap_or(std::ptr::null_mut())
}

unsafe extern "C" fn is_active(instance: *mut c_void) -> bool {
    unsafe { &*instance.cast::<Instance>() }
        .active
        .load(Ordering::Acquire)
}

unsafe extern "C" fn update(
    instance: *mut c_void,
    nodes: *const api::SemanticNode,
    len: usize,
) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let instance = unsafe { &mut *instance.cast::<Instance>() };
        let nodes = unsafe { std::slice::from_raw_parts(nodes, len) };
        let events = instance.adapter.update_if_active(|| {
            let mut root = Node::new(Role::Window);
            root.set_label("NeoWarp");
            root.set_children(nodes.iter().map(|node| NodeId(node.id)).collect::<Vec<_>>());
            let mut update = Vec::with_capacity(nodes.len() + 1);
            update.push((ROOT, root));
            for source in nodes {
                let mut node = Node::new(if source.flags & api::CHECKABLE != 0 {
                    Role::CheckBox
                } else if source.flags & api::EXPANDABLE != 0 {
                    Role::TreeItem
                } else {
                    Role::Button
                });
                let label = unsafe { std::slice::from_raw_parts(source.label, source.label_len) };
                node.set_label(String::from_utf8_lossy(label).as_ref());
                let [x0, y0, x1, y1] = source.bounds;
                node.set_bounds(accesskit::Rect::new(x0, y0, x1, y1));
                node.add_action(Action::Click);
                if source.flags & api::SELECTED != 0 {
                    node.set_selected(true);
                }
                if source.flags & api::CHECKABLE != 0 {
                    node.set_toggled(if source.flags & api::CHECKED != 0 {
                        accesskit::Toggled::True
                    } else {
                        accesskit::Toggled::False
                    });
                }
                if source.flags & api::EXPANDABLE != 0 {
                    node.set_expanded(source.flags & api::EXPANDED != 0);
                    node.add_action(Action::Expand);
                    node.add_action(Action::Collapse);
                }
                update.push((NodeId(source.id), node));
            }
            TreeUpdate {
                nodes: update,
                tree: Some(TreeInfo::new(ROOT)),
                tree_id: TreeId::ROOT,
                focus: ROOT,
            }
        });
        if let Some(events) = events {
            events.raise();
        }
        true
    }))
    .unwrap_or(false)
}

unsafe extern "C" fn destroy(instance: *mut c_void) {
    // Called on the window thread before its HWND is destroyed.
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        drop(Box::from_raw(instance.cast::<Instance>()))
    }));
}

static API: api::AccessibilityApi = api::AccessibilityApi {
    header: api::PluginHeader {
        abi_version: api::ABI_VERSION,
        capability: api::ACCESSIBILITY,
        size: std::mem::size_of::<api::AccessibilityApi>(),
    },
    create,
    is_active,
    update,
    destroy,
};

#[unsafe(no_mangle)]
pub extern "C" fn neowarp_plugin_v1() -> *const api::PluginHeader {
    &API.header
}
