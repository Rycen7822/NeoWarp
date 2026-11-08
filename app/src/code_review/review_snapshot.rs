//! One window's last review, retaining text and interaction state but no editor models.
use super::*;

const BODY_BUDGET: usize = 1024 * 1024;

struct SavedCard {
    entry: FileChangeEntry,
    expanded: bool,
    auto_collapse_pending: bool,
    state: Option<ReviewEditorState>,
    body: Option<(Box<str>, Box<str>)>,
}

pub(crate) struct ReviewSnapshot {
    root: PathBuf,
    revision: (EntityId, u64),
    mode: DiffMode,
    merge_base: Option<String>,
    wrap_lines: bool,
    files: Vec<SavedCard>,
    statuses: HashMap<String, GitFileStatus>,
    scroll_index: usize,
    scroll_offset: Pixels,
}

impl GitReviewView {
    pub(crate) fn snapshot(&mut self, ctx: &mut ViewContext<Self>) -> Option<ReviewSnapshot> {
        let summary_ready = self.refreshing.is_none()
            && !self.refresh_pending
            && (self.mode == DiffMode::Head || self.merge_base.is_some());
        let pending = self.loading.as_ref().map(|(path, _)| path.clone());
        self.cancel_loading();
        if let Some(task) = self.refreshing.take() {
            task.abort();
        }
        let revision = self
            .status
            .as_ref(ctx)
            .review_revision(ctx)
            .filter(|_| summary_ready);
        let mut remaining = BODY_BUDGET;
        let files = self
            .files
            .drain(..)
            .map(|mut card| {
                let editor = card.editor.take();
                let state = editor
                    .as_ref()
                    .map(|editor| editor.as_ref(ctx).review_state(ctx))
                    .or(card.editor_state);
                let body = if revision.is_some()
                    && pending.as_ref() != Some(&card.entry.path)
                    && (!card.dirty || card.cached_current.is_some())
                {
                    card.base.as_ref().and_then(|base| {
                        if let Some(current) = card.cached_current.take() {
                            if base.len() + current.len() <= remaining {
                                remaining -= base.len() + current.len();
                                return Some((base.as_str().into(), current));
                            }
                            return None;
                        }
                        let model = editor.as_ref()?.as_ref(ctx).model.as_ref(ctx);
                        let buffer = model.buffer().as_ref(ctx);
                        // The initial block marker occupies one byte outside plain text.
                        let current_len = buffer.max_byte_offset().as_usize().saturating_sub(1);
                        if base.len() + current_len > remaining {
                            return None;
                        }
                        let current = buffer.text().into_string().into_boxed_str();
                        if base.len() + current.len() > remaining {
                            return None;
                        }
                        remaining -= base.len() + current.len();
                        Some((base.as_str().into(), current))
                    })
                } else {
                    None
                };
                SavedCard {
                    entry: card.entry,
                    expanded: card.expanded,
                    auto_collapse_pending: card.auto_collapse_pending,
                    state,
                    body,
                }
            })
            .collect();
        Some(ReviewSnapshot {
            root: self.root.clone(),
            revision: revision?,
            mode: self.mode.clone(),
            merge_base: self.merge_base.take(),
            wrap_lines: self.wrap_lines,
            files,
            statuses: std::mem::take(&mut self.file_statuses),
            scroll_index: self.list.get_scroll_index(),
            scroll_offset: self.list.get_scroll_offset(),
        })
    }

    pub(super) fn restore_snapshot(
        &mut self,
        snapshot: ReviewSnapshot,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        if snapshot.root != self.root
            || Some(snapshot.revision) != self.status.as_ref(ctx).review_revision(ctx)
        {
            return false;
        }
        self.mode = snapshot.mode;
        self.merge_base = snapshot.merge_base;
        self.wrap_lines = snapshot.wrap_lines;
        self.file_statuses = snapshot.statuses;
        self.selector.update(ctx, |selector, ctx| {
            selector.set_mode(self.mode.clone(), ctx)
        });
        self.set_files(
            snapshot
                .files
                .iter()
                .map(|file| file.entry.clone())
                .collect(),
            ctx,
        );
        for (card, saved) in self.files.iter_mut().zip(snapshot.files) {
            card.expanded = saved.expanded;
            card.auto_collapse_pending = saved.auto_collapse_pending;
            card.editor_state = saved.state;
            if let Some((base, current)) = saved.body {
                card.base = Some(Arc::new(base.into_string()));
                card.cached_current = Some(current);
            }
        }
        if !self.files.is_empty() {
            self.list.scroll_to_with_offset(
                snapshot.scroll_index.min(self.files.len() - 1),
                snapshot.scroll_offset,
            );
        }
        true
    }
}
