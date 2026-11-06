use std::time::Duration;

use instant::Instant;
use warp_core::ui::theme::Fill;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::elements::{Align, Container, Empty, Flex, MouseStateHandle, ParentElement};
use warpui::fonts::Weight;
use warpui::keymap::FixedBinding;
use warpui::modals::{AlertDialogWithCallbacks, AppModalCallback, ModalButton};
use warpui::platform::Cursor;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use crate::appearance::Appearance;
use crate::localization::tr;
use crate::ui_components::dialog::{Dialog, dialog_styles};
use crate::window_settings::WindowSettings;

pub(super) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings(vec![
        FixedBinding::new("escape", NativeModalAction::Close, id!("NativeModal")),
        FixedBinding::new("enter", NativeModalAction::Confirm, id!("NativeModal")),
    ]);
}

/// Application dialog above a [`super::Workspace`]. The first button is [`ButtonVariant::Accent`].
pub struct NativeModal {
    alert_dialog: Option<AlertDialogWithCallbacks<AppModalCallback>>,
    modal_button_mouse_states: Vec<MouseStateHandle>,
    close_confirmation: Option<(Instant, SpawnedFutureHandle)>,
    close_approved: bool,
}

#[derive(Debug)]
pub enum NativeModalAction {
    /// Trigger a callback registered in [`NativeModal::alert_dialog`] and reset the modal.
    TriggerButtonCallback(usize),
    /// Triggers the last button in the list, as we assume the last button is "cancel".
    Close,
    /// Triggers the first button in the list, as we assume the first button is "confirm".
    Confirm,
}

pub enum NativeModalEvent {
    Close,
}

impl NativeModal {
    pub fn new(_ctx: &mut ViewContext<Self>) -> Self {
        NativeModal {
            alert_dialog: None,
            modal_button_mouse_states: Default::default(),
            close_confirmation: None,
            close_approved: false,
        }
    }

    pub fn set_alert_dialog(&mut self, alert_dialog: AlertDialogWithCallbacks<AppModalCallback>) {
        self.reset();
        for _ in 0..alert_dialog.button_data.len() {
            self.modal_button_mouse_states.push(Default::default());
        }
        self.alert_dialog = Some(alert_dialog);
    }

    fn reset(&mut self) {
        if let Some((_, timer)) = self.close_confirmation.take() {
            timer.abort();
        }
        self.close_approved = false;
        self.alert_dialog = None;
        self.modal_button_mouse_states = Default::default();
    }

    pub(super) fn confirm_window_close(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        if std::mem::take(&mut self.close_approved)
            || !*WindowSettings::as_ref(ctx).confirm_before_closing
        {
            return true;
        }
        if self.alert_dialog.is_some() {
            return false;
        }
        let seconds = *WindowSettings::as_ref(ctx).close_confirmation_timeout;
        self.set_alert_dialog(AlertDialogWithCallbacks::for_app(
            tr("Close this window?"),
            "",
            vec![
                ModalButton::for_app(tr("Close completely"), |_| {}),
                ModalButton::for_app(tr("Cancel"), |_| {}),
            ],
            |_| {},
        ));
        let timeout = Duration::from_secs(u64::from(seconds));
        let deadline = Instant::now() + timeout;
        self.schedule_close_countdown(deadline, ctx);
        ctx.notify();
        false
    }

    fn schedule_close_countdown(&mut self, deadline: Instant, ctx: &mut ViewContext<Self>) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            self.reset();
            ctx.emit(NativeModalEvent::Close);
            ctx.notify();
            return;
        }
        self.close_confirmation = Some((
            deadline,
            ctx.spawn(
                Timer::after(remaining.min(Duration::from_secs(1))),
                move |modal, _, ctx| {
                    if modal
                        .close_confirmation
                        .as_ref()
                        .is_some_and(|(current, _)| *current == deadline)
                    {
                        modal.schedule_close_countdown(deadline, ctx);
                        ctx.notify();
                    }
                },
            ),
        ));
    }

    fn trigger_button_callback(&mut self, idx: usize, ctx: &mut ViewContext<Self>) {
        if self
            .alert_dialog
            .as_ref()
            .is_none_or(|dialog| idx >= dialog.button_data.len())
        {
            return;
        }
        if let Some((deadline, _)) = self.close_confirmation.as_ref() {
            let approved = idx == 0 && Instant::now() < *deadline;
            self.reset();
            self.close_approved = approved;
            ctx.emit(NativeModalEvent::Close);
            if approved {
                ctx.close_window();
            }
            return;
        }
        // Once we trigger a callback from a button, we are guaranteed that the modal will close so
        // it's ok to take the alert dialog from Self.
        if let Some(mut dialog) = self.alert_dialog.take() {
            let button = dialog.button_data.remove(idx);
            (button.on_click)(ctx);
        }
        self.reset();
        ctx.emit(NativeModalEvent::Close);
    }
}

impl Drop for NativeModal {
    fn drop(&mut self) {
        self.reset();
    }
}

impl Entity for NativeModal {
    type Event = NativeModalEvent;
}

impl View for NativeModal {
    fn ui_name() -> &'static str {
        "NativeModal"
    }

    fn render(&self, app: &warpui::AppContext) -> Box<dyn warpui::Element> {
        let Some(alert_dialog) = self.alert_dialog.as_ref() else {
            log::warn!("No alert dialog was set for the native modal");
            return Empty::new().finish();
        };
        let appearance = Appearance::as_ref(app);
        let button_style = UiComponentStyles {
            font_size: Some(14.),
            font_weight: Some(Weight::Bold),
            width: Some(240.),
            height: Some(40.),
            ..Default::default()
        };

        let mut dialog_column_contents = Vec::new();

        for (i, modal_button) in alert_dialog.button_data.iter().enumerate() {
            let button = Align::new(
                appearance
                    .ui_builder()
                    .button(
                        if i == 0 {
                            ButtonVariant::Accent
                        } else {
                            ButtonVariant::Basic
                        },
                        self.modal_button_mouse_states
                            .get(i)
                            .expect("Modal button mouse state should be set")
                            .clone(),
                    )
                    .with_centered_text_label(modal_button.title.clone())
                    .with_style(button_style)
                    .build()
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(NativeModalAction::TriggerButtonCallback(i))
                    })
                    .finish(),
            )
            .finish();

            dialog_column_contents.push(Container::new(button).with_padding_bottom(8.).finish());
        }

        let dialog_column = Flex::column()
            .with_children(dialog_column_contents)
            .finish();
        let info_text = if let Some((deadline, _)) = &self.close_confirmation {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
            tr("Confirm within {seconds}s, or closing will be cancelled.")
                .replace("{seconds}", &seconds.to_string())
        } else {
            alert_dialog.info_text.clone()
        };
        let dialog = Dialog::new(
            alert_dialog.message_text.clone(),
            Some(info_text),
            UiComponentStyles {
                width: Some(280.),
                padding: Some(Coords::uniform(24.)),
                ..dialog_styles(appearance)
            },
        )
        .with_child(dialog_column)
        .build()
        .finish();

        // This blurs the background and makes it uninteractable.
        Container::new(Align::new(dialog).finish())
            .with_background_color(Fill::blur().into())
            .with_corner_radius(app.windows().window_corner_radius())
            .finish()
    }
}

impl TypedActionView for NativeModal {
    type Action = NativeModalAction;

    fn handle_action(&mut self, action: &NativeModalAction, ctx: &mut ViewContext<Self>) {
        match action {
            NativeModalAction::TriggerButtonCallback(idx) => {
                self.trigger_button_callback(*idx, ctx);
            }
            NativeModalAction::Close => {
                if let Some(last_button_idx) = self.modal_button_mouse_states.len().checked_sub(1) {
                    self.trigger_button_callback(last_button_idx, ctx);
                }
            }
            NativeModalAction::Confirm => {
                self.trigger_button_callback(0, ctx);
            }
        }
    }
}
