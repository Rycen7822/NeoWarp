//! Local tools for external CLI agents, independent of the retired built-in Agent.

use std::time::Duration;

use warpui::r#async::Timer;
use warpui::elements::{
    Border, ChildView, Clipped, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult,
    EventHandler, Expanded, Flex, Hoverable, MainAxisAlignment, MouseStateHandle, ParentElement,
    Shrinkable, Text, Wrap,
};
use warpui::text_layout::ClipConfig;
use warpui::{AppContext, Element, SingletonEntity, ViewContext, ViewHandle};

use super::{
    ContextMenuAction, ContextMenuState, ContextMenuType, PromptPart, PromptPosition,
    TerminalAction, TerminalModel, TerminalView,
};
use crate::appearance::Appearance;
use crate::context_chips::ContextChipKind;
use crate::context_chips::display_chip::{GitLineChanges, render_git_diff_stats_content};
use crate::context_chips::prompt::Prompt;
use crate::context_chips::prompt_type::PromptType;
use crate::editor::{EditorOptions, EditorView, Event as EditorEvent};
use crate::localization::tr;
use crate::menu::{MenuItem, MenuItemFields};
use crate::pane_group::PaneGroupAction;
use crate::project_sessions::config::Provider;
use crate::terminal::cli_toolbelt_config::CliToolbeltItem as Item;
use crate::terminal::model::BlockId;
use crate::terminal::session_settings::SessionSettings;
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
    mouse: Vec<(Item, MouseStateHandle)>,
}

impl CliToolbelt {
    pub(super) fn has_rich_input(&self) -> bool {
        self.mouse.iter().any(|(item, _)| *item == Item::RichInput)
    }
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
            mouse: Vec::new(),
        });
        self.sync_cli_toolbelt_layout(ctx);
        self.set_cli_toolbelt_prompt_active(true, ctx);
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
            self.set_cli_toolbelt_prompt_active(false, ctx);
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
            if !state.mouse.iter().any(|(item, _)| *item == Item::RichInput) {
                return;
            }
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

    fn set_cli_toolbelt_prompt_active(&self, active: bool, ctx: &mut ViewContext<Self>) {
        self.current_prompt.update(ctx, |prompt, ctx| {
            if let PromptType::Dynamic { prompt } = prompt {
                prompt.update(ctx, |prompt, ctx| {
                    prompt.set_cli_toolbelt_active(active, ctx)
                });
            }
        });
    }

    pub(super) fn sync_cli_toolbelt_layout(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(state) = &mut self.cli_toolbelt else {
            return;
        };
        let (left, right) = SessionSettings::as_ref(ctx)
            .cli_agent_footer_chip_selection
            .items();
        state.mouse = left
            .into_iter()
            .chain(right)
            .map(|item| (item, MouseStateHandle::default()))
            .collect();
        if state.editor.is_some() && !state.mouse.iter().any(|(item, _)| *item == Item::RichInput) {
            self.toggle_cli_rich_input(ctx);
        }
        self.update_git_status_subscription(ctx);
        ctx.notify();
    }

    pub(super) fn show_cli_toolbelt_menu(
        &mut self,
        position: warpui::geometry::vector::Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.cli_toolbelt.is_none() {
            return;
        }
        let mut items = vec![
            MenuItemFields::new(tr("Copy prompt"))
                .with_on_select_action(TerminalAction::ContextMenu(ContextMenuAction::CopyPrompt {
                    position: PromptPosition::Input,
                    part: PromptPart::EntirePrompt,
                }))
                .into_item(),
        ];
        let mut kinds = Prompt::as_ref(ctx).chip_kinds();
        for kind in SessionSettings::as_ref(ctx)
            .cli_agent_footer_chip_selection
            .context_chips()
        {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        for kind in kinds {
            if self
                .current_prompt
                .as_ref(ctx)
                .latest_chip_value(&kind, ctx)
                .is_some()
                && let Some(chip) = kind.to_chip()
            {
                if items.len() == 1 {
                    items.push(MenuItem::Separator);
                }
                items.push(
                    MenuItemFields::new(format!("{} {}", tr("Copy"), tr(chip.title())))
                        .with_on_select_action(TerminalAction::ContextMenu(
                            ContextMenuAction::CopyPrompt {
                                position: PromptPosition::Input,
                                part: PromptPart::ContextChip(kind),
                            },
                        ))
                        .into_item(),
                );
            }
        }
        items.extend([
            MenuItem::Separator,
            MenuItemFields::new(tr("Edit CLI agent toolbelt"))
                .with_on_select_action(TerminalAction::EditCliToolbelt)
                .into_item(),
        ]);
        self.show_context_menu(
            ContextMenuState {
                menu_type: ContextMenuType::CliToolbelt { position },
            },
            items,
            ctx,
        );
    }

    pub(super) fn attach_cli_toolbelt_file(&self, ctx: &mut ViewContext<Self>) {
        let Some(state) = &self.cli_toolbelt else {
            return;
        };
        let block = state.block.clone();
        let weak = ctx.handle();
        ctx.open_file_picker(
            move |result, app| {
                let paths = match result {
                    Ok(paths) => paths,
                    Err(error) => {
                        log::warn!("Could not select a CLI attachment: {error}");
                        return;
                    }
                };
                let Some(view) = weak.upgrade(app) else {
                    return;
                };
                view.update(app, |view, ctx| {
                    if !view
                        .cli_toolbelt
                        .as_ref()
                        .is_some_and(|state| state.block == block)
                        || !view.project_session_is_running(&block)
                    {
                        return;
                    }
                    let mut paths = paths;
                    if let Some(distro) = view.active_session_wsl_distro(ctx) {
                        for path in &mut paths {
                            *path = match warp_util::path::parse_wsl_unc_path(std::path::Path::new(
                                path,
                            )) {
                                Some(unc) if unc.distro.eq_ignore_ascii_case(&distro) => {
                                    unc.linux_path
                                }
                                Some(_) => {
                                    log::warn!(
                                        "Cannot attach a file from another WSL distribution"
                                    );
                                    return;
                                }
                                None => warp_util::path::convert_windows_path_to_wsl(path),
                            };
                        }
                    }
                    let text = warpui::clipboard_utils::escaped_paths_str(
                        &paths,
                        Some(view.shell_family(ctx)),
                    );
                    if let Some(editor) = view
                        .cli_toolbelt
                        .as_ref()
                        .and_then(|state| state.editor.clone())
                    {
                        editor.update(ctx, |editor, ctx| {
                            editor.system_insert(
                                &text,
                                crate::editor::PlainTextEditorViewAction::SystemInsert,
                                ctx,
                            );
                        });
                    } else {
                        view.write_user_bytes_to_pty(text.into_bytes(), ctx);
                    }
                });
            },
            warpui::platform::FilePickerConfiguration::new(),
        );
    }

    fn render_cli_toolbelt_item(&self, item: &Item, app: &AppContext) -> Option<Box<dyn Element>> {
        let state = self.cli_toolbelt.as_ref()?;
        let mouse = state.mouse.iter().find(|(kind, _)| kind == item)?.1.clone();
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let color = theme.sub_text_color(theme.surface_1());
        let size = appearance.ui_font_size();
        let content = if *item == Item::ContextChip(ContextChipKind::GitDiffStats) {
            let metadata = self.git_status_metadata(app)?;
            render_git_diff_stats_content(
                &GitLineChanges::from_diff_stats(&metadata.stats_against_head),
                size,
                appearance.ui_font_family(),
                size,
                appearance,
            )
        } else {
            let (icon, text) = if let Item::ContextChip(kind) = item {
                let value = self
                    .current_prompt
                    .as_ref(app)
                    .latest_chip_value(kind, app)
                    .map(|value| value.to_string());
                let text = match (value, kind) {
                    (Some(value), _) => value,
                    (None, ContextChipKind::WorkingDirectory) => self
                        .active_block_metadata
                        .as_ref()?
                        .current_working_directory()?
                        .to_owned(),
                    (None, _) => return None,
                };
                (kind.udi_icon(), text)
            } else {
                let (label, icon) = item.control()?;
                let text = if *item == Item::RichInput {
                    format!(
                        "{}  Ctrl G",
                        tr(if state.editor.is_some() {
                            "Hide Rich Input"
                        } else {
                            label
                        })
                    )
                } else {
                    tr(label).to_owned()
                };
                (Some(icon), text)
            };
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.);
            if let Some(icon) = icon {
                row.add_child(
                    ConstrainedBox::new(icon.to_warpui_icon(color).finish())
                        .with_width(16.)
                        .with_height(16.)
                        .finish(),
                );
            }
            row.add_child(
                Shrinkable::new(
                    1.,
                    Text::new_inline(text, appearance.ui_font_family(), size)
                        .with_clip(ClipConfig::ellipsis())
                        .with_color(color.into())
                        .finish(),
                )
                .finish(),
            );
            ConstrainedBox::new(row.finish())
                .with_max_width(360.)
                .finish()
        };
        let action = TerminalAction::CliToolbeltItem(item.clone());
        Some(
            Hoverable::new(mouse, |mouse| {
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
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
            .finish(),
        )
    }

    pub(super) fn handle_cli_toolbelt_item(&mut self, item: &Item, ctx: &mut ViewContext<Self>) {
        if self.cli_toolbelt.is_none() {
            return;
        }
        match item {
            Item::FileExplorer => {
                ctx.dispatch_typed_action(&WorkspaceAction::FocusTerminalViewInWorkspace {
                    terminal_view_id: self.view_id,
                });
                ctx.dispatch_typed_action(&WorkspaceAction::SelectSidebarTab(SidebarTab::Files));
            }
            Item::RichInput => self.toggle_cli_rich_input(ctx),
            Item::FileAttach => self.attach_cli_toolbelt_file(ctx),
            Item::Settings => ctx.dispatch_typed_action(&WorkspaceAction::ShowSettings),
            Item::ContextChip(ContextChipKind::GitDiffStats) => ctx.dispatch_typed_action(
                &WorkspaceAction::OpenGitReview(self.current_repo_path.clone()),
            ),
            Item::ContextChip(ContextChipKind::WorkingDirectory) => {
                if let Some(path) = self.active_session_path_if_local(ctx) {
                    ctx.dispatch_typed_action(&WorkspaceAction::OpenInExplorer { path });
                }
            }
            Item::ContextChip(kind) => self.copy_prompt(
                &PromptPosition::Input,
                &PromptPart::ContextChip(kind.clone()),
                ctx,
            ),
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
        let (left, right) = SessionSettings::as_ref(app)
            .cli_agent_footer_chip_selection
            .items();
        let side = |items: Vec<Item>, alignment| {
            Wrap::row()
                .with_spacing(4.)
                .with_run_spacing(4.)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_alignment(alignment)
                .with_children(
                    items
                        .iter()
                        .filter_map(|item| self.render_cli_toolbelt_item(item, app)),
                )
                .finish()
        };
        let icon = match state.provider {
            Provider::Codex => Icon::OpenAILogo,
            Provider::Claude => Icon::ClaudeLogo,
            Provider::Pi => Icon::PiLogo,
            Provider::Hermes => Icon::HermesMonogram,
            Provider::Custom => Icon::Terminal,
        };
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(
                ConstrainedBox::new(
                    icon.to_warpui_icon(theme.sub_text_color(theme.surface_1()))
                        .finish(),
                )
                .with_width(16.)
                .with_height(16.)
                .finish(),
            )
            .with_child(Expanded::new(1., side(left, MainAxisAlignment::Start)).finish())
            .with_child(Expanded::new(1., side(right, MainAxisAlignment::End)).finish())
            .finish();
        let toolbar = EventHandler::new(
            Container::new(row)
                .with_horizontal_padding(16.)
                .with_vertical_padding(4.)
                .finish(),
        )
        .on_right_mouse_down(|ctx, _, position, _| {
            ctx.dispatch_typed_action(TerminalAction::ShowCliToolbeltMenu { position });
            DispatchEventResult::StopPropagation
        })
        .finish();
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
        column.add_child(toolbar);
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
