use gpui_kit::component::{
    button::*,
    input::{Input, InputState},
};
use gpui_kit::*;
use rodio::{DeviceSinkBuilder, Player};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
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
    Play,
    Pause,
    Stop,
    SeekTo(Duration),
}

struct Probe {
    search: Entity<InputState>,
    commands: mpsc::Sender<Command>,
    status: Arc<Mutex<String>>,
    playback: Arc<Mutex<MediaPlayback>>,
    media: Option<MediaControls>,
}

fn start_audio(
    status: Arc<Mutex<String>>,
    playback: Arc<Mutex<MediaPlayback>>,
) -> mpsc::Sender<Command> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            let runtime = tokio::runtime::Runtime::new()?;
            let device = DeviceSinkBuilder::open_default_sink()?;
            let player = Player::connect_new(device.mixer());
            loop {
                let progress = Some(MediaPosition(player.get_pos()));
                *playback.lock().unwrap() = if player.empty() {
                    MediaPlayback::Stopped
                } else if player.is_paused() {
                    MediaPlayback::Paused { progress }
                } else {
                    MediaPlayback::Playing { progress }
                };
                let command = match receive.recv_timeout(Duration::from_millis(100)) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
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
                        Command::Play => player.play(),
                        Command::Pause => player.pause(),
                        Command::Stop => player.stop(),
                        Command::SeekTo(position) => player.try_seek(position)?,
                    }
                    *status.lock().unwrap() = if player.empty() {
                        "Остановлено"
                    } else if player.is_paused() {
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

fn media_controls(
    window: &Window,
    commands: mpsc::Sender<Command>,
) -> Result<MediaControls, Box<dyn std::error::Error>> {
    let handle = raw_window_handle::HasWindowHandle::window_handle(window)?;
    let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("Win32 window required".into());
    };
    let mut media = MediaControls::new(PlatformConfig {
        dbus_name: "dagaz_probe",
        display_name: "Dagaz probe",
        hwnd: Some(handle.hwnd.get() as *mut std::ffi::c_void),
    })?;
    media.attach(move |event| {
        let command = match event {
            MediaControlEvent::Play => Command::Play,
            MediaControlEvent::Pause => Command::Pause,
            MediaControlEvent::Toggle => Command::Toggle,
            MediaControlEvent::Stop => Command::Stop,
            MediaControlEvent::SetPosition(position) => Command::SeekTo(position.0),
            _ => return,
        };
        let _ = commands.send(command);
    })?;
    media.set_metadata(MediaMetadata {
        title: Some("Dagaz Windows probe"),
        artist: Some("Synthetic fixtures"),
        duration: Some(Duration::from_secs(30)),
        ..Default::default()
    })?;
    media.set_playback(MediaPlayback::Stopped)?;
    Ok(media)
}

impl Probe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder("Проверка ввода и поиска кириллицы"));
        let status = Arc::new(Mutex::new("Готово".into()));
        let playback = Arc::new(Mutex::new(MediaPlayback::Stopped));
        let commands = start_audio(status.clone(), playback.clone());
        let media = match media_controls(window, commands.clone()) {
            Ok(media) => Some(media),
            Err(error) => {
                *status.lock().unwrap() = format!("Системные медиакнопки: {error}");
                None
            }
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if let Some(media) = &mut this.media
                            && let Err(error) =
                                media.set_playback(this.playback.lock().unwrap().clone())
                        {
                            *this.status.lock().unwrap() =
                                format!("Системная медиасессия: {error}");
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            search,
            commands,
            status,
            playback,
            media,
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
