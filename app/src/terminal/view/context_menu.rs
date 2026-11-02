use warpui::UpdateView;

use super::{
    CONTEXT_MENU_WIDTH, ContextMenuState, MenuItem, TerminalAction, TerminalView, ViewContext,
};

impl TerminalView {
    pub(super) fn show_context_menu(
        &mut self,
        menu_state: ContextMenuState,
        items: Vec<MenuItem<TerminalAction>>,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.update_view(&self.context_menu, |context_menu, view_ctx| {
            context_menu.set_origin(menu_state.menu_type.origin());
            context_menu.set_width(CONTEXT_MENU_WIDTH);
            // This will also reset the selection.
            context_menu.set_items(items, view_ctx);
        });

        self.context_menu_state = Some(menu_state);
        ctx.focus(&self.context_menu);
        ctx.notify();
    }
}
