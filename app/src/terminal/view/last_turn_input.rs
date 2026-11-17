use super::*;
use crate::code_review::last_turn::LastTurn;
use crate::project_sessions::ProjectSessions;

#[cfg(test)]
#[path = "last_turn_input_tests.rs"]
mod tests;

impl TerminalView {
    pub(crate) fn last_turn_review(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) -> Option<ModelHandle<LastTurn>> {
        if !ctx.has_singleton_model::<ProjectSessions>() {
            return None;
        }
        let source = ProjectSessions::as_ref(ctx).turn_source(ctx.view_id())?;
        let Some(LocalOrRemotePath::Local(root)) = self.current_repo_path.clone() else {
            return None;
        };
        if let Some(model) = &self.last_turn {
            let state = model.as_ref(ctx);
            if state.root == root
                && state.block == source.block
                && state.provider == source.provider()
            {
                return Some(model.clone());
            }
        }
        self.end_turn_recording(None, ctx);
        let terminal = ctx.view_id();
        let model = ctx.add_model(|ctx| LastTurn::new(root, source, terminal, ctx));
        self.last_turn = Some(model.clone());
        Some(model)
    }

    pub(super) fn submit_turn_input(
        &mut self,
        bytes: Vec<u8>,
        sync: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.last_turn_review(ctx);
        self.write_user_bytes_now(bytes, true, sync, ctx);
    }

    pub(super) fn end_turn_recording(
        &mut self,
        block: Option<&BlockId>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self
            .last_turn
            .as_ref()
            .is_some_and(|model| block.is_none_or(|block| *block == model.as_ref(ctx).block))
        {
            if let Some(model) = self.last_turn.take() {
                model.update(ctx, |model, ctx| model.end(ctx));
            }
            ctx.notify();
        }
    }
}
