use enum_iterator::{Sequence, all};
use itertools::{Either, Itertools};
use warpui::elements::{
    Align, Border, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CornerRadius, CrossAxisAlignment, Element, Expanded, Fill, Flex, MainAxisSize,
    MouseStateHandle, ParentElement, Radius, Shrinkable,
};
use warpui::keymap::{DescriptionContext, Keystroke};
use warpui::presenter::ChildView;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::units::Pixels;
use warpui::windowing::{StateEvent, WindowManager};
use warpui::{
    AppContext, Entity, FocusContext, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle, WindowId,
};

use super::utils::{
    BLOCKS_KEYBINDINGS, FUNDAMENTALS_KEYBINDINGS, INPUT_EDITOR_KEYBINDINGS, TERMINAL_KEYBINDINGS,
    get_additional_keybindings,
};
use crate::appearance::Appearance;
use crate::command_palette::PRIORITIZED_KEYBINDINGS;
use crate::editor::{
    EditorView, Event as EditorEvent, PropagateAndNoOpNavigationKeys, SingleLineEditorOptions,
    TextOptions,
};
use crate::search_bar::SearchBar;
use crate::settings_view;
use crate::settings_view::keybindings::{KeybindingChangedEvent, KeybindingChangedNotifier};
use crate::ui_components::buttons::icon_button;
use crate::ui_components::icons::Icon;
use crate::ui_components::window_focus_dimming::WindowFocusDimming;
use crate::util::bindings::{CommandBinding, filter_bindings_including_keystroke};
use crate::workspace::tab_settings::TabSettings;
use crate::workspace::{PANEL_HEADER_HEIGHT, WorkspaceAction};

const DESCRIPTION_FONT_SIZE: f32 = 14.;
const ITEM_PADDING_BOTTOM: f32 = 6.;
const SCROLLBAR_OFFSET: f32 = 7.;
const SCROLLBAR_WIDTH: warpui::elements::ScrollbarWidth = warpui::elements::ScrollbarWidth::Auto;
const SECTION_HEADER_FONT_SIZE: f32 = 16.;
const SECTION_SPACING: f32 = 12.;

const KEYBINDINGS_PAGE_SHORTCUT: &str = "workspace:toggle_keybindings_page";
const LINK_WIDTH: f32 = 30.;

#[derive(Default)]
struct MouseStateHandles {
    navigate_to_settings_link: MouseStateHandle,
    close: MouseStateHandle,
    header_dimming: MouseStateHandle,
}

pub struct KeybindingsView {
    /// List of all keybindings.
    bindings: Option<Vec<CommandBinding>>,
    /// List of keybindings based on search query.
    binding_results: Option<Vec<CommandBinding>>,
    clipped_scroll_state: ClippedScrollStateHandle,
    mouse_state_handles: MouseStateHandles,
    search_bar: ViewHandle<SearchBar>,
    search_editor: ViewHandle<EditorView>,
    window_id: WindowId,
}

/// Keybindings are sorted into these sections,
/// where "Fundamentals" is the default for any remaining non-categorized ones.
/// This should always align with documentation: https://docs.warp.dev/getting-started/keyboard-shortcuts
#[derive(Clone, Eq, PartialEq, Sequence)]
pub enum KeybindingSection {
    Essentials,
    Blocks,
    InputEditor,
    Terminal,
    Fundamentals,
}

#[derive(Debug)]
pub enum KeybindingsAction {}

impl KeybindingsView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let bindings = Some(Self::build_bindings(ctx));

        let search_editor = {
            let options = SingleLineEditorOptions {
                text: TextOptions::ui_font_size(),
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            };
            ctx.add_typed_action_view(|ctx| EditorView::single_line(options, ctx))
        };

        ctx.subscribe_to_view(&search_editor, move |me, _, event, ctx| {
            me.handle_search_editor_event(event, ctx);
        });

        search_editor.update(ctx, |editor, ctx| {
            editor.clear_buffer_and_reset_undo_stack(ctx);
            editor.set_placeholder_text(settings_view::keybindings::SEARCH_PLACEHOLDER, ctx);
        });

        let search_bar = {
            let style = UiComponentStyles {
                border_radius: Some(CornerRadius::with_all(Radius::Percentage(20.))),
                margin: Some(Coords {
                    top: SECTION_SPACING,
                    bottom: SECTION_SPACING,
                    left: SECTION_SPACING + SCROLLBAR_OFFSET,
                    right: SECTION_SPACING + SCROLLBAR_OFFSET,
                }),
                ..Default::default()
            };
            ctx.add_typed_action_view(|_| {
                let mut search_bar = SearchBar::new(search_editor.clone());
                search_bar.with_style(style);
                search_bar
            })
        };

        let bindings_notifier = KeybindingChangedNotifier::handle(ctx);
        ctx.subscribe_to_model(&bindings_notifier, |me, _, event, ctx| {
            me.handle_keybinding_changed(event, ctx);
        });

        // Rebuild bindings when layout-dependent settings change, so dynamic
        // descriptions (e.g. "Close tabs below" under vertical tabs) stay in
        // sync while the panel is open. Other surfaces repopulate every time
        // they're opened; this one is built once per panel lifetime.
        let tab_settings_handle = TabSettings::handle(ctx);
        ctx.observe(&tab_settings_handle, Self::rebuild_bindings);

        let window_state = WindowManager::handle(ctx);
        ctx.subscribe_to_model(&window_state, |_, _, event, ctx| {
            let StateEvent::ValueChanged { current, previous } = event;
            if WindowManager::did_window_change_focus(ctx.window_id(), current, previous) {
                ctx.notify();
            }
        });

        Self {
            bindings: bindings.clone(),
            binding_results: bindings,
            clipped_scroll_state: Default::default(),
            mouse_state_handles: Default::default(),
            search_bar,
            search_editor,
            window_id: ctx.window_id(),
        }
    }

    fn build_bindings(ctx: &AppContext) -> Vec<CommandBinding> {
        ctx.get_key_bindings()
            .filter_map(|lens| CommandBinding::from_lens(lens, ctx))
            .chain(get_additional_keybindings())
            .filter(|a| {
                a.trigger.is_some()
                    && !a
                        .description
                        .in_context(DescriptionContext::Default)
                        .is_empty()
            })
            .sorted_by(|a, b| {
                a.description
                    .in_context(DescriptionContext::Default)
                    .cmp(b.description.in_context(DescriptionContext::Default))
            })
            .dedup_by(|a, b| a.description == b.description)
            .collect()
    }

    fn rebuild_bindings(
        &mut self,
        _tab_settings: ModelHandle<TabSettings>,
        ctx: &mut ViewContext<Self>,
    ) {
        let bindings = Self::build_bindings(ctx);
        // Preserve any active search filter so the user doesn't lose their
        // query position just because they toggled a layout setting.
        let search_term = self.search_editor.as_ref(ctx).buffer_text(ctx);
        let filtered: Vec<CommandBinding> = filter_bindings_including_keystroke(
            bindings.iter(),
            &search_term,
            DescriptionContext::Default,
        )
        .map(|(_, binding)| binding.clone())
        .collect();
        self.bindings = Some(bindings);
        self.binding_results = Some(filtered);
        ctx.notify();
    }

    fn handle_keybinding_changed(
        &mut self,
        event: &KeybindingChangedEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            KeybindingChangedEvent::BindingChanged {
                binding_name,
                new_trigger,
            } => {
                let visible_binding_updated = update_binding_with_new_trigger(
                    &mut self.bindings,
                    binding_name,
                    new_trigger.clone(),
                ) && update_binding_with_new_trigger(
                    &mut self.binding_results,
                    binding_name,
                    new_trigger.clone(),
                );
                if visible_binding_updated || binding_name == KEYBINDINGS_PAGE_SHORTCUT {
                    ctx.notify();
                }
            }
        }
    }

    fn handle_search_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(_) => {
                let search_term = self.search_editor.as_ref(ctx).buffer_text(ctx);
                self.binding_results = Some(
                    filter_bindings_including_keystroke(
                        self.bindings.iter().flatten(),
                        &search_term,
                        DescriptionContext::Default,
                    )
                    .map(|orig| orig.1.clone())
                    .collect(),
                );

                self.clipped_scroll_state.scroll_to(Pixels::zero());
                ctx.notify();
            }
            EditorEvent::Escape => {
                ctx.emit(KeybindingsEvent::Escape);
            }
            _ => {}
        }
    }

    /// Returns a list of sorted command bindings belonging to the given section.
    /// Bindings that aren't already categorized are added to the "Fundamentals" section.
    fn get_bindings_by_section(
        &self,
        section: KeybindingSection,
    ) -> impl Iterator<Item = CommandBinding> + use<> {
        let bindings = self
            .binding_results
            .as_ref()
            .expect("Should have command bindings vector");

        let binding_list = match section {
            KeybindingSection::Essentials => PRIORITIZED_KEYBINDINGS,
            KeybindingSection::Blocks => BLOCKS_KEYBINDINGS,
            KeybindingSection::InputEditor => INPUT_EDITOR_KEYBINDINGS,
            KeybindingSection::Terminal => TERMINAL_KEYBINDINGS,
            KeybindingSection::Fundamentals => FUNDAMENTALS_KEYBINDINGS,
        };

        let filtered_bindings = bindings.iter().filter_map(|binding| {
            // Return bindings that match those listed in their corresponding section
            if binding_list.iter().any(|&x| x == binding.name) {
                Some(binding.clone())
            } else {
                None
            }
        });

        let categorized_bindings = [
            PRIORITIZED_KEYBINDINGS,
            BLOCKS_KEYBINDINGS,
            INPUT_EDITOR_KEYBINDINGS,
            TERMINAL_KEYBINDINGS,
            FUNDAMENTALS_KEYBINDINGS,
        ]
        .concat();

        // Return non-categorized bindings to the "Fundamentals" section
        let extended_iterator = if section == KeybindingSection::Fundamentals {
            let remaining_bindings = bindings.iter().filter_map(|binding| {
                if categorized_bindings.contains(&binding.name.as_str()) {
                    None
                } else {
                    // Return binding if not found in any categories
                    Some(binding.clone())
                }
            });
            Either::Left(filtered_bindings.chain(remaining_bindings))
        } else {
            Either::Right(filtered_bindings)
        };

        extended_iterator.sorted_by(|a, b| {
            a.description
                .in_context(DescriptionContext::Default)
                .cmp(b.description.in_context(DescriptionContext::Default))
        })
    }

    /// Helper function to render wrappable text given different text styles.
    /// Use override_style to further customize.
    fn render_text(
        &self,
        text: String,
        override_style: Option<UiComponentStyles>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        // Default text style
        let mut style = UiComponentStyles {
            font_size: Some(DESCRIPTION_FONT_SIZE),
            font_color: Some(
                appearance
                    .theme()
                    .sub_text_color(appearance.theme().background())
                    .into(),
            ),
            ..Default::default()
        };

        if let Some(override_style) = override_style {
            style = style.merge(override_style);
        }

        appearance
            .ui_builder()
            .wrappable_text(text, true)
            .with_style(style)
            .build()
            .finish()
    }

    fn render_subheader(&self, appearance: &Appearance) -> Box<dyn Element> {
        let bindings = self
            .bindings
            .as_ref()
            .expect("Should have command bindings vector");

        let mut column = Flex::column();

        // If there is a valid keybinding set that opens this panel, display it
        // to the user.
        if let Some(keystroke) = bindings
            .iter()
            .find(|&binding| binding.name == KEYBINDINGS_PAGE_SHORTCUT)
            .and_then(|shortcut| shortcut.trigger.as_ref())
        {
            let keybinding_row = Flex::row()
                .with_child(
                    appearance
                        .ui_builder()
                        .keyboard_shortcut(keystroke)
                        .with_style(UiComponentStyles {
                            margin: Some(Coords {
                                left: 0.0,
                                right: SCROLLBAR_OFFSET,
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                        .build()
                        .finish(),
                )
                .with_child(self.render_text("To toggle this panel".into(), None, appearance))
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .finish();

            column.add_child(
                Container::new(keybinding_row)
                    .with_padding_bottom(SECTION_SPACING)
                    .finish(),
            );
        }

        let settings_link = ConstrainedBox::new(
            appearance
                .ui_builder()
                .link(
                    "here.".into(),
                    None,
                    Some(Box::new(|ctx| {
                        ctx.dispatch_typed_action(WorkspaceAction::ConfigureKeybindingSettings {
                            keybinding_name: None,
                        });
                    })),
                    self.mouse_state_handles.navigate_to_settings_link.clone(),
                )
                .soft_wrap(false)
                .with_style(UiComponentStyles {
                    font_size: Some(DESCRIPTION_FONT_SIZE),
                    ..Default::default()
                })
                .build()
                .finish(),
        )
        .with_max_width(LINK_WIDTH)
        .finish();

        Container::new(
            column
                .with_child(self.render_text(
                    "Go to settings > keyboard shortcuts to configure custom keybindings".into(),
                    None,
                    appearance,
                ))
                .with_child(settings_link)
                .finish(),
        )
        .with_uniform_padding(SECTION_SPACING)
        .with_margin_bottom(SECTION_SPACING)
        .with_margin_left(SCROLLBAR_OFFSET)
        .finish()
    }

    /// Returns a list of rendered bindings within the given section, else None if section is empty.
    fn render_section(
        &self,
        section: KeybindingSection,
        appearance: &Appearance,
    ) -> Option<Box<dyn Element>> {
        let mut bindings = self.get_bindings_by_section(section.clone()).peekable();

        // Don't render section if there are no bindings to show.
        bindings.peek()?;

        let mut binding_list =
            Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

        let title = match section {
            KeybindingSection::Essentials => "Essentials",
            KeybindingSection::Blocks => "Blocks",
            KeybindingSection::InputEditor => "Input Editor",
            KeybindingSection::Terminal => "Terminal",
            KeybindingSection::Fundamentals => "Fundamentals",
        };

        let mut section_header = self.render_text(
            title.into(),
            Some(UiComponentStyles {
                font_color: Some(appearance.theme().active_ui_text_color().into()),
                font_size: Some(SECTION_HEADER_FONT_SIZE),
                ..Default::default()
            }),
            appearance,
        );

        section_header = Container::new(section_header)
            .with_margin_bottom(ITEM_PADDING_BOTTOM)
            .with_uniform_padding(SECTION_SPACING)
            .with_padding_left(SECTION_SPACING + SCROLLBAR_OFFSET)
            .with_background(appearance.theme().surface_2())
            .with_border(
                Border::top(1.)
                    .with_border_color(appearance.theme().split_pane_border_color().into()),
            )
            .finish();

        binding_list.add_child(section_header);

        for binding in bindings {
            let mut binding_row = Flex::row();

            let label = self.render_text(
                binding
                    .description
                    .in_context(DescriptionContext::Default)
                    .to_string(),
                None,
                appearance,
            );
            binding_row.add_child(
                Shrinkable::new(
                    1.,
                    Align::new(Container::new(label).finish()).left().finish(),
                )
                .finish(),
            );
            if let Some(trigger) = binding.trigger.clone() {
                let shortcut = appearance.ui_builder().keyboard_shortcut(&trigger).build();
                binding_row.add_child(Container::new(shortcut.finish()).finish());
            }

            let mut binding_row = Container::new(
                binding_row
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .finish(),
            );
            binding_row = binding_row
                .with_uniform_margin(SECTION_SPACING)
                .with_margin_left(SECTION_SPACING + SCROLLBAR_OFFSET);

            binding_list.add_child(binding_row.finish())
        }

        Some(binding_list.finish())
    }

    fn render_header(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let title = appearance
            .ui_builder()
            .wrappable_text("Keyboard Shortcuts", false)
            .with_style(UiComponentStyles {
                font_size: Some(16.),
                font_weight: Some(warpui::fonts::Weight::Semibold),
                ..Default::default()
            })
            .build()
            .finish();
        let close = icon_button(
            appearance,
            Icon::X,
            false,
            self.mouse_state_handles.close.clone(),
        )
        .build()
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::ToggleKeybindingsPage))
        .finish();
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1., title).finish())
            .with_child(close)
            .finish();
        let header = ConstrainedBox::new(
            Container::new(row)
                .with_horizontal_padding(12.)
                .with_vertical_padding(5.)
                .finish(),
        )
        .with_height(PANEL_HEADER_HEIGHT)
        .finish();
        WindowFocusDimming::apply_panel_header_dimming(
            header,
            self.mouse_state_handles.header_dimming.clone(),
            appearance.theme().surface_1().into(),
            self.window_id,
            app,
        )
    }

    fn render_body(&self, appearance: &Appearance) -> Box<dyn Element> {
        let keybinding_sections = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_children(
                all::<KeybindingSection>()
                    .filter_map(|section| self.render_section(section, appearance))
                    .map(|child| {
                        Container::new(child)
                            .with_margin_bottom(SECTION_SPACING)
                            .finish()
                    }),
            );

        ClippedScrollable::vertical(
            self.clipped_scroll_state.clone(),
            keybinding_sections.finish(),
            SCROLLBAR_WIDTH,
            appearance
                .theme()
                .disabled_text_color(appearance.theme().background())
                .into(),
            appearance
                .theme()
                .main_text_color(appearance.theme().background())
                .into(),
            Fill::None,
        )
        .finish()
    }
}

#[derive(PartialEq, Eq)]
pub enum KeybindingsEvent {
    Escape,
}

impl Entity for KeybindingsView {
    type Event = KeybindingsEvent;
}

impl TypedActionView for KeybindingsView {
    type Action = KeybindingsAction;
}

impl View for KeybindingsView {
    fn ui_name() -> &'static str {
        "Keybindings"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.search_editor);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let search_bar = ChildView::new(&self.search_bar).finish();
        let subheader = self.render_subheader(appearance);
        let body = self.render_body(appearance);

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.render_header(appearance, app))
            .with_child(search_bar)
            .with_child(subheader)
            .with_child(Shrinkable::new(1., body).finish())
            .with_main_axis_size(MainAxisSize::Max)
            .finish()
    }
}

pub fn update_binding_with_new_trigger(
    bindings: &mut Option<Vec<CommandBinding>>,
    name: &str,
    trigger: Option<Keystroke>,
) -> bool {
    let bindings = match bindings {
        None => return false,
        Some(bindings) => bindings,
    };

    match bindings.iter_mut().find(|binding| binding.name == name) {
        Some(binding) => {
            binding.trigger = trigger;
            true
        }
        None => false,
    }
}
