use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::code_review::last_turn::LastTurn;
use crate::project_sessions::ProjectSessions;

#[cfg(test)]
#[path = "last_turn_input_tests.rs"]
mod tests;

struct Input {
    bytes: Vec<u8>,
    submit: bool,
    sync: bool,
}

pub(super) struct Pending {
    block: BlockId,
    first: Input,
    queue: VecDeque<Input>,
    bytes: usize,
    cancelled: Arc<AtomicBool>,
    task: SpawnedFutureHandle,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.task.abort();
    }
}

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
        let missed = self.last_turn_missed.as_ref() == Some(&source.block);
        let model = ctx.add_model(|ctx| LastTurn::new(root, source, terminal, missed, ctx));
        self.last_turn = Some(model.clone());
        Some(model)
    }

    pub(super) fn submit_turn_input(
        &mut self,
        bytes: Vec<u8>,
        sync: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.defer_turn_input(&bytes, true, sync, ctx) {
            return;
        }
        self.begin_turn_input(
            Input {
                bytes,
                submit: true,
                sync,
            },
            VecDeque::new(),
            ctx,
        );
    }

    fn begin_turn_input(
        &mut self,
        first: Input,
        queue: VecDeque<Input>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(model) = self.last_turn_review(ctx) else {
            self.last_turn_missed = Some(self.model.lock().active_block_id().clone());
            self.write_user_bytes_now(first.bytes, first.submit, first.sync, ctx);
            self.drain_turn_inputs(queue, ctx);
            return;
        };
        if !model.as_ref(ctx).needs_capture() {
            self.write_user_bytes_now(first.bytes, first.submit, first.sync, ctx);
            self.drain_turn_inputs(queue, ctx);
            return;
        }
        let block = self.model.lock().active_block_id().clone();
        let job = model.update(ctx, |model, ctx| model.prepare(ctx));
        let cancelled = job.cancelled.clone();
        let callback_flag = cancelled.clone();
        let task = ctx.spawn(job.run(), move |me, result, ctx| {
            if me
                .last_turn_input
                .as_ref()
                .is_none_or(|pending| !Arc::ptr_eq(&pending.cancelled, &callback_flag))
            {
                return;
            }
            let Some(mut pending) = me.last_turn_input.take() else {
                return;
            };
            if *me.model.lock().active_block_id() != pending.block {
                return;
            }
            match result {
                Ok(prepared) => model.update(ctx, |model, ctx| model.sent(prepared, ctx)),
                Err(error) => {
                    model.update(ctx, |model, ctx| model.failed(format!("{error:#}"), ctx))
                }
            }
            let bytes = std::mem::take(&mut pending.first.bytes);
            me.write_user_bytes_now(bytes, true, pending.first.sync, ctx);
            let queue = std::mem::take(&mut pending.queue);
            drop(pending);
            me.drain_turn_inputs(queue, ctx);
            ctx.notify();
        });
        let bytes = first.bytes.len() + queue.iter().map(|input| input.bytes.len()).sum::<usize>();
        self.last_turn_input = Some(Pending {
            block,
            first,
            queue,
            bytes,
            cancelled,
            task,
        });
        ctx.notify();
    }

    fn drain_turn_inputs(&mut self, mut queue: VecDeque<Input>, ctx: &mut ViewContext<Self>) {
        while let Some(input) = queue.pop_front() {
            if input.submit {
                self.begin_turn_input(input, queue, ctx);
                return;
            }
            self.write_user_bytes_now(input.bytes, false, input.sync, ctx);
        }
    }

    pub(super) fn defer_turn_input(
        &mut self,
        bytes: &[u8],
        submit: bool,
        sync: bool,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(pending) = &self.last_turn_input else {
            return false;
        };
        if *self.model.lock().active_block_id() != pending.block {
            self.cancel_turn_input(tr("Session changed").into(), ctx);
            return false;
        }
        if bytes == [3] {
            self.cancel_turn_input(tr("Input cancelled").into(), ctx);
            return false;
        }
        if pending.bytes.saturating_add(bytes.len()) > 64 * 1024 {
            // Fail open without truncating or reordering user input.
            let mut pending = self
                .last_turn_input
                .take()
                .expect("pending input was checked");
            pending.cancelled.store(true, Ordering::Relaxed);
            pending.task.abort();
            if let Some(model) = &self.last_turn {
                model.update(ctx, |model, ctx| {
                    model.failed(tr("Input queue limit reached").into(), ctx)
                });
            }
            let first = std::mem::take(&mut pending.first.bytes);
            self.write_user_bytes_now(first, true, pending.first.sync, ctx);
            for input in std::mem::take(&mut pending.queue) {
                self.write_user_bytes_now(input.bytes, input.submit, input.sync, ctx);
            }
            ctx.notify();
            return false;
        }
        let pending = self
            .last_turn_input
            .as_mut()
            .expect("pending input was checked");
        pending.bytes += bytes.len();
        pending.queue.push_back(Input {
            bytes: bytes.to_vec(),
            submit,
            sync,
        });
        true
    }

    pub(super) fn cancel_turn_input(&mut self, message: String, ctx: &mut ViewContext<Self>) {
        if self.last_turn_input.take().is_some()
            && let Some(model) = &self.last_turn
        {
            model.update(ctx, |model, ctx| model.failed(message, ctx));
            ctx.notify();
        }
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
            self.last_turn_input = None;
            if let Some(model) = self.last_turn.take() {
                model.update(ctx, |model, ctx| model.end(ctx));
            }
            ctx.notify();
        }
    }
}
