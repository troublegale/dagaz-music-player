use dagaz_windows::{ClientError, ServerConnection};
use gpui_kit::{
    component::{
        Disableable,
        button::*,
        input::{Input, InputState},
    },
    *,
};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};

struct Outcome {
    connection: Option<Arc<ServerConnection>>,
    address: Option<String>,
    message: String,
    error: bool,
}
pub struct Dagaz {
    address: Entity<InputState>,
    code: Entity<InputState>,
    name: Entity<InputState>,
    runtime: Arc<tokio::runtime::Runtime>,
    profile: PathBuf,
    send: mpsc::SyncSender<Outcome>,
    connection: Option<Arc<ServerConnection>>,
    busy: bool,
    message: String,
    error: bool,
}
impl Dagaz {
    pub fn new(
        profile: PathBuf,
        runtime: Arc<tokio::runtime::Runtime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://music.example.com"));
        let code = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Одноразовый код")
                .masked(true)
        });
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value("Этот компьютер", window, cx);
            state
        });
        let (send, receive) = mpsc::sync_channel(1);
        let task_send = send.clone();
        let task_profile = profile.clone();
        runtime.spawn(async move {
            let outcome = match ServerConnection::restore(&task_profile) {
                Ok(Some(connection)) => check(Arc::new(connection)).await,
                Ok(None) => Outcome {
                    connection: None,
                    address: None,
                    message: "Введи адрес своего сервера и код привязки.".into(),
                    error: false,
                },
                Err(error) => Outcome {
                    connection: None,
                    address: None,
                    message: error.to_string(),
                    error: true,
                },
            };
            let _ = task_send.send(outcome);
        });
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if let Ok(outcome) = receive.try_recv() {
                    if this
                        .update_in(cx, |this, window, cx| {
                            if let Some(address) = outcome.address {
                                this.address
                                    .update(cx, |state, cx| state.set_value(address, window, cx));
                            }
                            if outcome.connection.is_some() {
                                this.code
                                    .update(cx, |state, cx| state.set_value("", window, cx));
                            }
                            this.connection = outcome.connection;
                            this.message = outcome.message;
                            this.error = outcome.error;
                            this.busy = false;
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                } else if this.upgrade().is_none() {
                    break;
                }
            }
        })
        .detach();
        Self {
            address,
            code,
            name,
            runtime,
            profile,
            send,
            connection: None,
            busy: true,
            message: "Проверяем сохранённое подключение…".into(),
            error: false,
        }
    }
    fn pair(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let url = self.address.read(cx).value().trim().parse();
        let code = self.code.read(cx).value().trim().to_owned();
        let name = self.name.read(cx).value().to_string();
        let Ok(url) = url else {
            self.message = ClientError::InvalidAddress.to_string();
            self.error = true;
            cx.notify();
            return;
        };
        self.busy = true;
        self.error = false;
        self.message = "Подключаемся…".into();
        cx.notify();
        let send = self.send.clone();
        let profile = self.profile.clone();
        self.runtime.spawn(async move {
            let result = async {
                let connection = ServerConnection::pair(url, &code, &name).await?;
                connection.save(&profile)?;
                Ok::<_, ClientError>(Arc::new(connection))
            }
            .await;
            let outcome = match result {
                Ok(connection) => Outcome {
                    address: Some(connection.base_url().as_str().into()),
                    connection: Some(connection),
                    message: "Подключено".into(),
                    error: false,
                },
                Err(error) => Outcome {
                    connection: None,
                    address: None,
                    message: error.to_string(),
                    error: true,
                },
            };
            let _ = send.send(outcome);
        });
    }
    fn retry(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let Some(connection) = self.connection.clone() {
            let send = self.send.clone();
            self.busy = true;
            self.message = "Проверяем подключение…".into();
            cx.notify();
            self.runtime.spawn(async move {
                let _ = send.send(check(connection).await);
            });
        }
    }
}
async fn check(connection: Arc<ServerConnection>) -> Outcome {
    let address = Some(connection.base_url().as_str().into());
    match connection.check_access().await {
        Ok(()) => Outcome {
            connection: Some(connection),
            address,
            message: "Подключено".into(),
            error: false,
        },
        Err(ClientError::PairingRequired) => Outcome {
            connection: None,
            address,
            message: ClientError::PairingRequired.to_string(),
            error: true,
        },
        Err(error) => Outcome {
            connection: Some(connection),
            address,
            message: error.to_string(),
            error: true,
        },
    }
}
impl Render for Dagaz {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = rgb(0x91a0b4);
        let accent = rgb(0x9ce6c6);
        let mut content = div()
            .w(px(460.))
            .flex()
            .flex_col()
            .gap_4()
            .p_8()
            .rounded_xl()
            .bg(rgb(0x1c2531));
        if let Some(connection) = &self.connection {
            content = content
                .child(div().text_2xl().child("Мой сервер"))
                .child(
                    div()
                        .text_color(muted)
                        .child(connection.base_url().as_str().to_owned()),
                )
                .child(
                    div()
                        .text_color(muted)
                        .child(format!("Устройство: {}", connection.device_id())),
                )
                .child(
                    Button::new("retry")
                        .label("Проверить подключение")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                )
                .child(
                    Button::new("pair-again")
                        .label("Привязать заново")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connection = None;
                            this.message = "Введи новый одноразовый код.".into();
                            this.error = false;
                            cx.notify();
                        })),
                );
        } else {
            content = content
                .child(div().text_2xl().child("Твоя музыка начинается здесь"))
                .child(
                    div()
                        .text_color(muted)
                        .child("Подключи Dagaz к своему музыкальному серверу."),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child("Адрес сервера")
                        .child(Input::new(&self.address).disabled(self.busy)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child("Код привязки")
                        .child(Input::new(&self.code).disabled(self.busy)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child("Имя устройства")
                        .child(Input::new(&self.name).disabled(self.busy)),
                )
                .child(
                    Button::new("connect")
                        .primary()
                        .label(if self.busy {
                            "Подключаемся…"
                        } else {
                            "Подключить"
                        })
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.pair(cx))),
                );
        }
        content = content.child(
            div()
                .text_sm()
                .text_color(if self.error { rgb(0xf2b3a9) } else { muted })
                .child(self.message.clone()),
        );
        div()
            .size_full()
            .bg(rgb(0x151b24))
            .text_color(rgb(0xe8edf3))
            .font_family("Segoe UI")
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(76.))
                    .px_8()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(0x2b3543))
                    .child(div().text_2xl().text_color(accent).child("◇  dagaz")),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .child(content),
            )
    }
}
