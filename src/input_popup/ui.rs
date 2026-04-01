use std::error::Error;
use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, App, AppContext as _, Application, Bounds, Context, Element, ElementId, Entity,
    FocusHandle, GlobalElementId, InspectorElementId, InteractiveElement as _, IntoElement,
    KeyBinding, LayoutId, ParentElement as _, Pixels, Render, SharedString, Styled as _,
    Subscription, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
    actions, div, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::radio::Radio;
use gpui_component::scroll::ScrollableElement;
use gpui_component::{ActiveTheme, Disableable, IconName, Root, h_flex, v_flex};

use super::schema::{PopupInputRequest, PopupInputResponse, is_other_label};

const WINDOW_WIDTH: f32 = 600.0;
const WINDOW_MIN_WIDTH: f32 = 600.0;
const WINDOW_MIN_HEIGHT: f32 = 120.0;
const WINDOW_BOOTSTRAP_HEIGHT: f32 = 240.0;
const CONTENT_MAX_HEIGHT: f32 = 800.0;
const FOOTER_HEIGHT: f32 = 88.0;

actions!(popup_input, [Cancel, Submit]);

pub fn show_popup_dialog(request: PopupInputRequest) -> Result<PopupInputResponse, Box<dyn Error>> {
    let request = request.normalized();
    let shared_response = Arc::new(Mutex::new(None));
    let shared_error = Arc::new(Mutex::new(None));

    Application::new().run({
        let request = request.clone();
        let shared_response = Arc::clone(&shared_response);
        let shared_error = Arc::clone(&shared_error);

        move |cx: &mut App| {
            gpui_component::init(cx);
            cx.bind_keys([
                KeyBinding::new("escape", Cancel, None),
                KeyBinding::new("enter", Submit, None),
                KeyBinding::new("ctrl-enter", Submit, None),
                KeyBinding::new("cmd-enter", Submit, None),
            ]);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(
                None,
                size(px(WINDOW_WIDTH), px(WINDOW_BOOTSTRAP_HEIGHT)),
                cx,
            );
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(WINDOW_MIN_WIDTH), px(WINDOW_MIN_HEIGHT))),
                kind: WindowKind::Normal,
                titlebar: None,
                window_background: WindowBackgroundAppearance::Transparent,
                ..Default::default()
            };

            let open_window_result = cx.open_window(options, {
                let request = request.clone();
                let shared_response = Arc::clone(&shared_response);
                move |window, cx| {
                    cx.activate(false);
                    let view = cx.new(|cx| PopupView::new(request, shared_response, window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                }
            });

            if let Err(error) = open_window_result {
                if let Ok(mut slot) = shared_error.lock() {
                    *slot = Some(format!("failed to open gpui popup window: {error}"));
                }
                cx.quit();
            }
        }
    });

    if let Some(error) = shared_error
        .lock()
        .map_err(|error| format!("failed to read popup error: {error}"))?
        .clone()
    {
        eprintln!("[input-popup-debug] shared error before return: {error}");
        return Err(error.into());
    }

    let response = shared_response
        .lock()
        .map_err(|error| format!("failed to read popup result: {error}"))?
        .clone()
        .unwrap_or_else(PopupInputResponse::cancelled);
    eprintln!(
        "[input-popup-debug] show_popup_dialog returning: {:?}",
        response
    );
    Ok(response)
}

struct PopupView {
    request: PopupInputRequest,
    questions: Vec<QuestionState>,
    shared_response: Arc<Mutex<Option<PopupInputResponse>>>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

struct QuestionState {
    selected_option: Option<usize>,
    custom_input: Entity<InputState>,
}

impl PopupView {
    fn new(
        request: PopupInputRequest,
        shared_response: Arc<Mutex<Option<PopupInputResponse>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);

        let mut questions = Vec::with_capacity(request.questions.len());
        let mut subscriptions = Vec::with_capacity(request.questions.len());

        for (question_index, _) in request.questions.iter().enumerate() {
            let custom_input = cx.new(|cx| InputState::new(window, cx));
            custom_input.update(cx, |state, cx| {
                state.set_placeholder("Enter a custom answer", window, cx);
            });
            subscriptions.push(cx.subscribe(
                &custom_input,
                move |this: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let has_custom_answer = !this.questions[question_index]
                            .custom_input
                            .read(cx)
                            .value()
                            .trim()
                            .is_empty();
                        if has_custom_answer {
                            this.questions[question_index].selected_option =
                                this.custom_option_index(question_index);
                        }
                        cx.notify();
                    }
                },
            ));
            questions.push(QuestionState {
                selected_option: None,
                custom_input,
            });
        }

        Self {
            request,
            questions,
            shared_response,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    fn select_option(
        &mut self,
        question_index: usize,
        option_index: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.questions[question_index].selected_option = Some(option_index);
        cx.notify();
    }

    fn select_custom_option(&mut self, question_index: usize, cx: &mut Context<Self>) {
        self.questions[question_index].selected_option = self.custom_option_index(question_index);
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.finish(PopupInputResponse::cancelled(), window, cx);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_complete(cx) {
            return;
        }

        self.finish(self.build_response(cx), window, cx);
    }

    fn finish(
        &mut self,
        response: PopupInputResponse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        eprintln!(
            "[input-popup-debug] finish called with response: {:?}",
            response
        );
        if let Ok(mut slot) = self.shared_response.lock() {
            *slot = Some(response);
        }

        window.remove_window();
        cx.quit();
    }

    fn on_cancel_action(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel(window, cx);
    }

    fn on_submit_action(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        self.submit(window, cx);
    }

    fn custom_answer(&self, question_index: usize, cx: &App) -> Option<String> {
        let value = self.questions[question_index].custom_input.read(cx).value();
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    fn custom_option_index(&self, question_index: usize) -> Option<usize> {
        self.request.questions[question_index]
            .options
            .iter()
            .rposition(|option| is_other_label(&option.label))
    }

    fn is_complete(&self, cx: &App) -> bool {
        self.questions
            .iter()
            .enumerate()
            .all(|(question_index, state)| {
                self.custom_answer(question_index, cx).is_some()
                    || state.selected_option.is_some_and(|option_index| {
                        Some(option_index) != self.custom_option_index(question_index)
                    })
            })
    }

    fn build_response(&self, cx: &App) -> PopupInputResponse {
        PopupInputResponse::from_answers(self.questions.iter().enumerate().filter_map(
            |(question_index, state)| {
                let question = &self.request.questions[question_index];
                let answer = match state.selected_option {
                    Some(option_index)
                        if Some(option_index) == self.custom_option_index(question_index) =>
                    {
                        self.custom_answer(question_index, cx)?
                    }
                    Some(option_index) => question.options[option_index].label.clone(),
                    None => self.custom_answer(question_index, cx)?,
                };
                Some((question.id.clone(), answer))
            },
        ))
    }

    fn render_question(&self, question_index: usize, cx: &mut Context<Self>) -> AnyElement {
        let question = &self.request.questions[question_index];
        let state = &self.questions[question_index];

        let mut card = v_flex().w_full().gap_3().child(
            div()
                .text_xl()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(cx.theme().foreground)
                .child(question.question.clone()),
        );

        if question_index > 0 {
            card = card.pt_6().border_t_1().border_color(cx.theme().border);
        }

        let custom_option_index = self.custom_option_index(question_index);

        for (option_index, option) in question.options.iter().enumerate() {
            if Some(option_index) == custom_option_index {
                continue;
            }

            let selected = state.selected_option == Some(option_index);
            let option_id = (
                SharedString::from(format!("popup-option-{question_index}")),
                option_index,
            );

            card = card.child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.select_option(question_index, option_index, window, cx);
                        }),
                    )
                    .child(
                        Radio::new(option_id)
                            .checked(selected)
                            .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                                if *checked {
                                    this.select_option(question_index, option_index, window, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_lg()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(cx.theme().muted_foreground)
                            .child(option.description.clone()),
                    ),
            );
        }

        if custom_option_index.is_some() {
            let custom_selected = state.selected_option == custom_option_index;
            let custom_option_id = (
                SharedString::from(format!("popup-option-{question_index}")),
                custom_option_index.unwrap_or_default(),
            );

            card = card.child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.select_custom_option(question_index, cx);
                        }),
                    )
                    .child(
                        Radio::new(custom_option_id)
                            .checked(custom_selected)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if *checked {
                                    this.select_custom_option(question_index, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.select_custom_option(question_index, cx);
                                }),
                            )
                            .child(Input::new(&state.custom_input).w_full()),
                    ),
            );
        }

        card.into_any_element()
    }

    fn render_dialog(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let submit_disabled = !self.is_complete(cx);

        let mut content = v_flex().w_full().gap_6();

        for question_index in 0..self.questions.len() {
            content = content.child(self.render_question(question_index, cx));
        }

        div()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_cancel_action))
            .on_action(cx.listener(Self::on_submit_action))
            .w_full()
            .h_auto()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded_xl()
            .shadow_lg()
            .overflow_hidden()
            .child(
                v_flex()
                    .w_full()
                    .h_auto()
                    .child(
                        h_flex()
                            .w_full()
                            .justify_end()
                            .items_center()
                            .px_4()
                            .pt_3()
                            .pb_1()
                            .child(
                                Button::new("dialog-close")
                                    .ghost()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.cancel(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .h_auto()
                            .max_h(px(CONTENT_MAX_HEIGHT))
                            .px_6()
                            .pb_5()
                            .child(div().w_full().overflow_y_scrollbar().child(content)),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .flex_shrink_0()
                            .min_h(px(FOOTER_HEIGHT))
                            .justify_end()
                            .items_center()
                            .gap_3()
                            .px_6()
                            .py_4()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().secondary)
                            .child(Button::new("cancel").label("Cancel").on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.cancel(window, cx);
                                },
                            )))
                            .child(
                                Button::new("submit")
                                    .primary()
                                    .label("Submit")
                                    .disabled(submit_disabled)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for PopupView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        AutoResizeDialog::new(self.render_dialog(cx))
    }
}

struct AutoResizeDialog {
    child: AnyElement,
}

impl AutoResizeDialog {
    fn new(child: AnyElement) -> Self {
        Self { child }
    }
}

impl IntoElement for AutoResizeDialog {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AutoResizeDialog {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);

        let target_height = bounds.size.height.max(px(WINDOW_MIN_HEIGHT));
        let current_height = window.viewport_size().height;
        if (current_height - target_height).abs() > px(1.0) {
            window.on_next_frame(move |window, _| {
                window.resize(size(px(WINDOW_WIDTH), target_height));
            });
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
