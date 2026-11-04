use settings::Setting;
use warp_errors::report_if_error;
use warpui::keymap::FixedBinding;
use warpui::{AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use super::cli_toolbelt_config::{
    CLIAgentToolbarChipSelection as Selection, CliToolbeltItem as Item,
};
use super::session_settings::{SessionSettings, SessionSettingsChangedEvent};
use crate::appearance::Appearance;
use crate::chip_configurator::{
    ChipConfigurator, ChipConfiguratorAction, ChipConfiguratorLayout, ChipEditorModalConfig,
    ChipEditorMouseHandles, ChipEditorSectionsConfig, ConfigurableItem, ControlItemRenderer,
    render_chip_editor_modal, render_chip_editor_sections,
};
use crate::context_chips::ChipAvailability;
use crate::context_chips::renderer::Renderer;
use crate::localization::tr;

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;
    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        Action::Cancel,
        id!(CliToolbeltEditor::ui_name()),
    )]);
}

pub(crate) enum Event {
    Close,
    Error(String),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Action {
    Cancel,
    Save,
    Reset,
    Chip(ChipConfiguratorAction),
    Activate,
}

pub(crate) struct CliToolbeltEditor {
    chips: ChipConfigurator,
    mouse: ChipEditorMouseHandles,
    dirty: bool,
}

pub(crate) struct CliToolbeltInlineEditor {
    chips: ChipConfigurator,
    mouse: ChipEditorMouseHandles,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum InlineAction {
    Chip(ChipConfiguratorAction),
    Reset,
    Activate,
}

impl CliToolbeltEditor {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        Self {
            chips: chips_from_settings(ctx),
            mouse: Default::default(),
            dirty: false,
        }
    }
}

fn chips_from_settings(app: &AppContext) -> ChipConfigurator {
    let mut chips = ChipConfigurator::new(ChipConfiguratorLayout::LeftRightZones);
    load(
        &mut chips,
        &SessionSettings::as_ref(app).cli_agent_footer_chip_selection,
        app,
    );
    chips
}

fn load(chips: &mut ChipConfigurator, selection: &Selection, app: &AppContext) {
    let (left, right) = selection.items();
    let used: Vec<_> = left.iter().chain(&right).cloned().collect();
    let build = |item: Item| match item {
        Item::ContextChip(kind) => {
            Renderer::default_from_kind(kind, ChipAvailability::Enabled, Appearance::as_ref(app))
                .map(|renderer| ConfigurableItem::ContextChip(Box::new(renderer)))
        }
        control => {
            let (label, icon) = control.control()?;
            Some(ConfigurableItem::Control(
                ControlItemRenderer::new_with_label_and_icon(label.into(), icon).with_identifier(
                    serde_json::to_string(&control).expect("toolbar item serializes"),
                ),
            ))
        }
    };
    chips.reset();
    chips.left_chips = left.into_iter().filter_map(build).collect();
    chips.right_chips = right.into_iter().filter_map(build).collect();
    chips.unused_chips = Item::available()
        .into_iter()
        .filter(|item| !used.contains(item))
        .filter_map(build)
        .collect();
}

fn selection(chips: &ChipConfigurator) -> Selection {
    let decode = |item: &ConfigurableItem| match item {
        ConfigurableItem::ContextChip(renderer) => {
            Some(Item::ContextChip(renderer.chip_kind().clone()))
        }
        ConfigurableItem::Control(renderer) => serde_json::from_str(renderer.identifier()?).ok(),
    };
    let left: Vec<_> = chips.left_chips.iter().filter_map(decode).collect();
    let right: Vec<_> = chips.right_chips.iter().filter_map(decode).collect();
    if (left.clone(), right.clone()) == Selection::Default.items() {
        Selection::Default
    } else {
        Selection::Custom { left, right }
    }
}

fn save<V: View>(chips: &ChipConfigurator, ctx: &mut ViewContext<V>) -> anyhow::Result<()> {
    let selection = selection(chips);
    SessionSettings::handle(ctx).update(ctx, |settings, ctx| {
        settings
            .cli_agent_footer_chip_selection
            .set_value(selection, ctx)
    })
}

impl Entity for CliToolbeltEditor {
    type Event = Event;
}

impl TypedActionView for CliToolbeltEditor {
    type Action = Action;
    fn handle_action(&mut self, action: &Action, ctx: &mut ViewContext<Self>) {
        match action {
            Action::Cancel => ctx.emit(Event::Close),
            Action::Save => {
                if self.dirty
                    && let Err(error) = save(&self.chips, ctx)
                {
                    ctx.emit(Event::Error(error.to_string()));
                    return;
                }
                ctx.emit(Event::Close);
            }
            Action::Reset => {
                load(&mut self.chips, &Selection::Default, ctx);
                self.dirty = true;
            }
            Action::Chip(action) => {
                self.dirty |= self.chips.handle_action(action, ctx);
            }
            Action::Activate => (),
        }
        ctx.notify();
    }
}

impl View for CliToolbeltEditor {
    fn ui_name() -> &'static str {
        "CliToolbeltEditor"
    }
    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        render_chip_editor_modal(
            &self.chips,
            ChipEditorModalConfig {
                title: tr("Edit CLI agent toolbelt"),
                available_section_label: "Available chips",
                is_at_defaults: selection(&self.chips) == Selection::Default,
                is_dirty: self.dirty,
                cancel_action: Action::Cancel,
                save_action: Action::Save,
                reset_action: Action::Reset,
                activate_action: Action::Activate,
                chip_action_wrapper: Action::Chip,
                mouse_handles: &self.mouse,
            },
            Appearance::as_ref(app),
        )
    }
}

impl CliToolbeltInlineEditor {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let editor = Self {
            chips: chips_from_settings(ctx),
            mouse: Default::default(),
        };
        ctx.subscribe_to_model(&SessionSettings::handle(ctx), |me, _, event, ctx| {
            if matches!(
                event,
                SessionSettingsChangedEvent::CLIAgentToolbarChipSelectionSetting { .. }
            ) && me.chips.current_dragging_state.is_none()
            {
                load(
                    &mut me.chips,
                    &SessionSettings::as_ref(ctx).cli_agent_footer_chip_selection,
                    ctx,
                );
                ctx.notify();
            }
        });
        editor
    }
}

impl Entity for CliToolbeltInlineEditor {
    type Event = ();
}

impl TypedActionView for CliToolbeltInlineEditor {
    type Action = InlineAction;

    fn handle_action(&mut self, action: &InlineAction, ctx: &mut ViewContext<Self>) {
        let changed = match action {
            InlineAction::Chip(action) => self.chips.handle_action(action, ctx),
            InlineAction::Reset => {
                load(&mut self.chips, &Selection::Default, ctx);
                true
            }
            InlineAction::Activate => false,
        };
        if changed {
            report_if_error!(save(&self.chips, ctx));
        }
        ctx.notify();
    }
}

impl View for CliToolbeltInlineEditor {
    fn ui_name() -> &'static str {
        "CliToolbeltInlineEditor"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        render_chip_editor_sections(
            &self.chips,
            ChipEditorSectionsConfig {
                available_section_label: "Available items",
                is_at_defaults: selection(&self.chips) == Selection::Default,
                reset_action: InlineAction::Reset,
                activate_action: InlineAction::Activate,
                chip_action_wrapper: InlineAction::Chip,
                mouse_handles: &self.mouse,
            },
            Appearance::as_ref(app),
        )
    }
}

#[cfg(test)]
#[path = "cli_toolbelt_editor_tests.rs"]
mod tests;
