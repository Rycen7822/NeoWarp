use settings::Setting;
use warpui::keymap::FixedBinding;
use warpui::{AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use super::cli_toolbelt_config::{
    CLIAgentToolbarChipSelection as Selection, CliToolbeltItem as Item,
};
use super::session_settings::SessionSettings;
use crate::appearance::Appearance;
use crate::chip_configurator::{
    ChipConfigurator, ChipConfiguratorAction, ChipConfiguratorLayout, ChipEditorModalConfig,
    ChipEditorMouseHandles, ConfigurableItem, ControlItemRenderer, render_chip_editor_modal,
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

impl CliToolbeltEditor {
    pub(crate) fn new(ctx: &mut ViewContext<Self>) -> Self {
        let mut editor = Self {
            chips: ChipConfigurator::new(ChipConfiguratorLayout::LeftRightZones),
            mouse: Default::default(),
            dirty: false,
        };
        editor.load(
            &SessionSettings::as_ref(ctx)
                .cli_agent_footer_chip_selection
                .clone(),
            ctx,
        );
        editor
    }

    fn load(&mut self, selection: &Selection, ctx: &mut ViewContext<Self>) {
        let (left, right) = selection.items();
        let used: Vec<_> = left.iter().chain(&right).cloned().collect();
        let build = |item: Item| match item {
            Item::ContextChip(kind) => Renderer::default_from_kind(
                kind,
                ChipAvailability::Enabled,
                Appearance::as_ref(ctx),
            )
            .map(|renderer| ConfigurableItem::ContextChip(Box::new(renderer))),
            control => {
                let (label, icon) = control.control()?;
                Some(ConfigurableItem::Control(
                    ControlItemRenderer::new_with_label_and_icon(label.into(), icon)
                        .with_identifier(
                            serde_json::to_string(&control).expect("toolbar item serializes"),
                        ),
                ))
            }
        };
        self.chips.reset();
        self.chips.left_chips = left.into_iter().filter_map(build).collect();
        self.chips.right_chips = right.into_iter().filter_map(build).collect();
        self.chips.unused_chips = Item::available()
            .into_iter()
            .filter(|item| !used.contains(item))
            .filter_map(build)
            .collect();
    }

    fn selection(&self) -> Selection {
        let decode = |item: &ConfigurableItem| match item {
            ConfigurableItem::ContextChip(renderer) => {
                Some(Item::ContextChip(renderer.chip_kind().clone()))
            }
            ConfigurableItem::Control(renderer) => {
                serde_json::from_str(renderer.identifier()?).ok()
            }
        };
        let left: Vec<_> = self.chips.left_chips.iter().filter_map(decode).collect();
        let right: Vec<_> = self.chips.right_chips.iter().filter_map(decode).collect();
        if (left.clone(), right.clone()) == Selection::Default.items() {
            Selection::Default
        } else {
            Selection::Custom { left, right }
        }
    }
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
                if self.dirty {
                    let selection = self.selection();
                    let result = SessionSettings::handle(ctx).update(ctx, |settings, ctx| {
                        settings
                            .cli_agent_footer_chip_selection
                            .set_value(selection, ctx)
                    });
                    if let Err(error) = result {
                        ctx.emit(Event::Error(error.to_string()));
                        return;
                    }
                }
                ctx.emit(Event::Close);
            }
            Action::Reset => {
                self.load(&Selection::Default, ctx);
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
                is_at_defaults: self.selection() == Selection::Default,
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
