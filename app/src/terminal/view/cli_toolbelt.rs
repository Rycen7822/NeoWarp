//! Local tools for external CLI agents, independent of the retired built-in Agent.

use std::time::Duration;

use warpui::r#async::Timer;
use warpui::elements::{
    Border, ChildView, Clipped, ConstrainedBox, Container, CrossAxisAlignment, Expanded, Flex,
    Hoverable, MouseStateHandle, ParentElement, Shrinkable, Text,
};
use warpui::text_layout::ClipConfig;
use warpui::{AppContext, Element, SingletonEntity, ViewContext, ViewHandle};

use super::{BlockMetadata, TerminalAction, TerminalModel, TerminalView};
use crate::appearance::Appearance;
use crate::context_chips::display_chip::{GitLineChanges, render_git_diff_stats_content};
use crate::editor::{EditorOptions, EditorView, Event as EditorEvent};
use crate::localization::tr;
use crate::pane_group::PaneGroupAction;
use crate::project_sessions::config::Provider;
use crate::terminal::model::BlockId;
use crate::ui_components::icons::Icon;
use crate::workspace::{SidebarTab, WorkspaceAction};

#[cfg(test)]
#[path = "cli_toolbelt_tests.rs"]
mod tests;

pub(super) struct CliToolbelt {
    block: BlockId,
    provider: Provider,
    editor: Option<ViewHandle<EditorView>>,
    draft: String,
    submitting: bool,
    mouse: [MouseStateHandle; 4],
}

impl TerminalView {
    pub(super) fn focus_cli_rich_input(&self, ctx: &mut ViewContext<Self>) {
        if let Some(editor) = self
            .cli_toolbelt
            .as_ref()
            .and_then(|state| state.editor.as_ref())
        {
            ctx.focus(editor);
        }
    }

    pub(super) fn start_cli_toolbelt(
        &mut self,
        block: BlockId,
        provider: Provider,
        ctx: &mut ViewContext<Self>,
    ) {
        if self
            .cli_toolbelt
            .as_ref()
            .is_some_and(|state| state.block == block)
        {
            return;
        }
        self.cli_toolbelt = Some(CliToolbelt {
            block,
            provider,
            editor: None,
            draft: String::new(),
            submitting: false,
            mouse: Default::default(),
        });
        self.update_git_status_subscription(ctx);
        ctx.notify();
    }

    pub(super) fn end_cli_toolbelt(
        &mut self,
        block: Option<&BlockId>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self
            .cli_toolbelt
            .as_ref()
            .is_some_and(|state| block.is_none_or(|block| state.block == *block))
        {
            let state = self.cli_toolbelt.take().expect("toolbelt exists");
            if state
                .editor
                .as_ref()
                .is_some_and(|editor| editor.is_self_or_child_focused(ctx))
            {
                ctx.focus_self();
            }
            self.update_git_status_subscription(ctx);
            ctx.notify();
        }
    }

    pub(super) fn toggle_cli_rich_input(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(state) = &mut self.cli_toolbelt else {
            return;
        };
        if let Some(editor) = state.editor.take() {
            state.draft = editor.as_ref(ctx).buffer_text(ctx);
            ctx.focus_self();
        } else {
            let editor = ctx.add_typed_action_view(|ctx| {
                EditorView::new(
                    EditorOptions {
                        autogrow: true,
                        soft_wrap: true,
                        ..Default::default()
                    },
                    ctx,
                )
            });
            let draft = std::mem::take(&mut state.draft);
            editor.update(ctx, |editor, ctx| {
                editor.system_reset_buffer_text(&draft, ctx)
            });
            ctx.subscribe_to_view(&editor, |me, _, event, ctx| match event {
                EditorEvent::Enter => me.submit_cli_rich_input(ctx),
                EditorEvent::Escape => me.toggle_cli_rich_input(ctx),
                _ => (),
            });
            ctx.focus(&editor);
            ctx.dispatch_typed_action(&PaneGroupAction::HandleFocusChange);
            state.editor = Some(editor);
        }
        ctx.notify();
    }

    pub(super) fn submit_cli_rich_input(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(state) = &self.cli_toolbelt else {
            return;
        };
        if state.submitting || !self.project_session_is_running(&state.block) {
            return;
        }
        let Some(editor) = state.editor.clone() else {
            return;
        };
        let text = editor.as_ref(ctx).buffer_text(ctx);
        if text.is_empty() {
            return;
        }
        let block = state.block.clone();
        let provider = state.provider;
        let (bytes, separate_enter) = submission(provider, &text);
        self.write_user_bytes_to_pty(bytes, ctx);
        editor.update(ctx, |editor, ctx| editor.system_reset_buffer_text("", ctx));
        if provider == Provider::Claude {
            self.cli_toolbelt
                .as_mut()
                .expect("toolbelt exists")
                .submitting = true;
            ctx.spawn(
                Timer::after(Duration::from_millis(50)),
                move |me, _, ctx| me.finish_cli_submission(&block, ctx),
            );
        } else if separate_enter {
            self.write_user_bytes_to_pty(vec![b'\r'], ctx);
        }
    }

    fn finish_cli_submission(&mut self, block: &BlockId, ctx: &mut ViewContext<Self>) {
        // A delayed Enter must never reach the shell or a replacement CLI command.
        if self
            .cli_toolbelt
            .as_ref()
            .is_some_and(|state| state.block == *block)
            && self.project_session_is_running(block)
        {
            self.write_user_bytes_to_pty(vec![b'\r'], ctx);
            self.cli_toolbelt
                .as_mut()
                .expect("toolbelt exists")
                .submitting = false;
            ctx.notify();
        }
    }

    pub(super) fn render_cli_toolbelt(
        &self,
        model: &TerminalModel,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let state = self.cli_toolbelt.as_ref().expect("toolbelt visible");
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let color = theme.sub_text_color(theme.surface_1());
        let size = appearance.ui_font_size();
        let icon = |icon: Icon| {
            ConstrainedBox::new(icon.to_warpui_icon(color).finish())
                .with_width(16.)
                .with_height(16.)
                .finish()
        };
        let label = |icon_kind, text: String| {
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.)
                .with_child(icon(icon_kind))
                .with_child(
                    Shrinkable::new(
                        1.,
                        Text::new_inline(text, appearance.ui_font_family(), size)
                            .with_clip(ClipConfig::ellipsis())
                            .with_color(color.into())
                            .finish(),
                    )
                    .finish(),
                )
                .finish()
        };
        let chip = |content, index: usize| {
            Hoverable::new(state.mouse[index].clone(), |mouse| {
                Container::new(content)
                    .with_horizontal_padding(6.)
                    .with_vertical_padding(3.)
                    .with_border(Border::all(1.).with_border_fill(theme.outline()))
                    .with_background(if mouse.is_hovered() {
                        theme.surface_2()
                    } else {
                        theme.surface_1()
                    })
                    .finish()
            })
        };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.);
        let terminal_view_id = self.view_id;
        row.add_child(icon(match state.provider {
            Provider::Codex => Icon::OpenAILogo,
            Provider::Claude => Icon::ClaudeLogo,
            Provider::Pi => Icon::PiLogo,
            Provider::Hermes => Icon::HermesMonogram,
            Provider::Custom => Icon::Terminal,
        }));
        row.add_child(
            chip(label(Icon::FileCopy, tr("File explorer").into()), 0)
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::FocusTerminalViewInWorkspace {
                        terminal_view_id,
                    });
                    ctx.dispatch_typed_action(WorkspaceAction::SelectSidebarTab(SidebarTab::Files));
                })
                .finish(),
        );
        row.add_child(
            chip(
                label(
                    Icon::TextInput,
                    format!(
                        "{}  Ctrl G",
                        tr(if state.editor.is_some() {
                            "Hide Rich Input"
                        } else {
                            "Rich Input"
                        })
                    ),
                ),
                1,
            )
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(TerminalAction::ToggleCliRichInput))
            .finish(),
        );
        let directory = self
            .active_block_metadata
            .as_ref()
            .and_then(BlockMetadata::current_working_directory);
        if let Some(directory) = directory {
            // Do not stat the path during rendering: WSL UNC I/O can block the UI thread.
            let path = self
                .active_block_session_id()
                .and_then(|id| self.sessions.as_ref(app).get(id))
                .and_then(|session| {
                    session
                        .launch_data()
                        .and_then(|data| data.maybe_convert_absolute_path(directory))
                });
            row.add_child(
                Shrinkable::new(
                    1.,
                    chip(label(Icon::Folder, directory.into()), 2)
                        .on_click(move |ctx, _, _| {
                            if let Some(path) = &path {
                                ctx.dispatch_typed_action(WorkspaceAction::OpenInExplorer {
                                    path: path.clone(),
                                });
                            }
                        })
                        .finish(),
                )
                .finish(),
            );
        }
        row.add_child(Expanded::new(1., Flex::row().finish()).finish());
        if let Some(metadata) = self.git_status_metadata(app) {
            let stats = GitLineChanges::from_diff_stats(&metadata.stats_against_head);
            let repo = self.current_repo_path.clone();
            row.add_child(
                chip(
                    render_git_diff_stats_content(
                        &stats,
                        size,
                        appearance.ui_font_family(),
                        size,
                        appearance,
                    ),
                    3,
                )
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::OpenGitReview(repo.clone()))
                })
                .finish(),
            );
        }
        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        if let Some(editor) = &state.editor {
            column.add_child(
                Container::new(
                    ConstrainedBox::new(Clipped::new(ChildView::new(editor).finish()).finish())
                        .with_max_height(180.)
                        .finish(),
                )
                .with_uniform_padding(12.)
                .finish(),
            );
        }
        column.add_child(
            Container::new(row.finish())
                .with_horizontal_padding(16.)
                .with_vertical_padding(4.)
                .finish(),
        );
        let background = if model.is_alt_screen_active() {
            model
                .alt_screen()
                .inferred_bg_color()
                .unwrap_or_else(|| theme.surface_1().into_solid())
        } else {
            theme.surface_1().into_solid()
        };
        Container::new(column.finish())
            .with_background(background)
            .with_border(Border::top(1.).with_border_fill(theme.outline()))
            .finish()
    }
}

// Same text/Enter framing as upstream's local CLI footer; no shell command execution.
fn submission(provider: Provider, text: &str) -> (Vec<u8>, bool) {
    match provider {
        Provider::Codex | Provider::Hermes => {
            (format!("\x1b[200~{text}\x1b[201~").into_bytes(), true)
        }
        Provider::Claude => (text.as_bytes().to_vec(), true),
        Provider::Pi | Provider::Custom => (format!("{text}\r").into_bytes(), false),
    }
}
