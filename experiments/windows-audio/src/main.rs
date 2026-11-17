use gpui_kit::component::{
    button::*,
    input::{Input, InputState},
};
use gpui_kit::*;
use rodio::{DeviceSinkBuilder, Player};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

enum Command {
    Local(PathBuf),
    Network,
    Toggle,
    Seek,
}

struct Probe {
    search: Entity<InputState>,
    commands: mpsc::Sender<Command>,
    status: Arc<Mutex<String>>,
}

fn start_audio(status: Arc<Mutex<String>>) -> mpsc::Sender<Command> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let runtime = tokio::runtime::Runtime::new()?;
            let device = DeviceSinkBuilder::open_default_sink()?;
            let player = Player::connect_new(device.mixer());
            while let Ok(command) = receive.recv() {
                let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                    match command {
                        Command::Local(path) => {
                            player.stop();
                            player.append(rodio::Decoder::try_from(std::fs::File::open(path)?)?);
                            player.play();
                        }
                        Command::Network => {
                            *status.lock().unwrap() = "Подкачка WAV…".into();
                            let run = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)?
                                .as_nanos();
                            let (reader, length) =
                                runtime.block_on(dagaz_windows_audio_probe::open_stream(
                                    &format!("http://127.0.0.1:18888/long.wav?drop=1&ui={run}"),
                                    Arc::new(std::sync::atomic::AtomicU64::new(0)),
                                ))?;
                            let decoder = rodio::Decoder::builder()
                                .with_data(reader)
                                .with_byte_len(length)
                                .with_seekable(true)
                                .build()?;
                            player.stop();
                            player.append(decoder);
                            player.play();
                        }
                        Command::Toggle => {
                            if player.is_paused() {
                                player.play();
                            } else {
                                player.pause();
                            }
                        }
                        Command::Seek => player.try_seek(Duration::from_secs(10))?,
                    }
                    *status.lock().unwrap() = if player.is_paused() {
                        "Пауза"
                    } else {
                        "Воспроизведение"
                    }
                    .into();
                    Ok(())
                })();
                if let Err(error) = result {
                    *status.lock().unwrap() = format!("Ошибка: {error}");
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            *status.lock().unwrap() = format!("Аудиоустройство: {error}");
        }
    });
    send
}

impl Probe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder("Проверка ввода и поиска кириллицы"));
        let status = Arc::new(Mutex::new("Готово".into()));
        let commands = start_audio(status.clone());
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            search,
            commands,
            status,
        }
    }
    fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            *self.status.lock().unwrap() = "Аудиопоток завершён".into();
        }
    }
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.search.read(cx).value().to_lowercase();
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio");
        div()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .child("Dagaz — проверка Windows")
            .child(Input::new(&self.search))
            .children(
                [
                    ("WAV — синус", "pcm.wav"),
                    ("MP3 — синус", "vbr.mp3"),
                    ("FLAC — синус", "tone.flac"),
                ]
                .into_iter()
                .enumerate()
                .filter(|(_, (label, _))| label.to_lowercase().contains(&filter))
                .map(|(index, (label, file))| {
                    let path = fixtures.join(file);
                    Button::new(("fixture", index)).label(label).on_click(
                        cx.listener(move |this, _, _, _| this.send(Command::Local(path.clone()))),
                    )
                }),
            )
            .child(
                Button::new("network")
                    .primary()
                    .label("Сетевой WAV с обрывом")
                    .on_click(cx.listener(|this, _, _, _| this.send(Command::Network))),
            )
            .child(
                Button::new("pause")
                    .label("Пауза / продолжить")
                    .on_click(cx.listener(|this, _, _, _| this.send(Command::Toggle))),
            )
            .child(
                Button::new("seek")
                    .label("На 10 секунд")
                    .on_click(cx.listener(|this, _, _, _| this.send(Command::Seek))),
            )
            .child(
                Button::new("file")
                    .label("Открыть файл…")
                    .on_click(cx.listener(|_, _, _, cx| {
                        let request = cx.prompt_for_paths(PathPromptOptions {
                            files: true,
                            directories: false,
                            multiple: false,
                            prompt: Some("Аудиофайл".into()),
                        });
                        cx.spawn(async move |this, cx| match request.await {
                            Ok(Ok(Some(paths))) => {
                                let _ = this.update(cx, |this, _| {
                                    if let Some(path) = paths.into_iter().next() {
                                        this.send(Command::Local(path));
                                    }
                                });
                            }
                            Ok(Ok(None)) => {}
                            error => {
                                let _ = this.update(cx, |this, _| {
                                    *this.status.lock().unwrap() =
                                        format!("Диалог файлов: {error:?}")
                                });
                            }
                        })
                        .detach();
                    })),
            )
            .child(self.status.lock().unwrap().clone())
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Probe::new(window, cx))
        })
        .expect("failed to open probe window");
    });
}
