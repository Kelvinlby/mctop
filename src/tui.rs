//! Terminal setup and the event loop.

use std::sync::Arc;
use std::time::Duration;

use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use tokio::sync::mpsc;
use tokio::time::{self, MissedTickBehavior};

use crate::app::{Action, App, Key};
use crate::config::Config;
use crate::source::{self, Control, Kind, Update};
use crate::ui::{self, theme::Theme};

/// How long the key reader waits before checking whether it should stop.
const POLL: Duration = Duration::from_millis(100);

/// Run the dashboard until the user quits.
pub async fn run(
    config: Arc<Config>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let theme = Theme::new(&config.ui);
    let mut app = App::new(Arc::clone(&config));

    match &config_path {
        Some(path) => app.note(Kind::Info, format!("config: {}", path.display())),
        None => app.note(
            Kind::Info,
            "no config file; using defaults — run `mctop config init` to write one",
        ),
    }

    let (updates_tx, mut updates_rx) = mpsc::channel::<Update>(256);
    let (controls_tx, controls_rx) = mpsc::channel::<Control>(8);
    source::spawn(Arc::clone(&config), updates_tx, controls_rx);

    let (keys_tx, mut keys_rx) = mpsc::channel::<Key>(64);
    let reader = spawn_key_reader(keys_tx);

    install_panic_hook();
    let mut terminal = ratatui::try_init()?;

    let mut draw = time::interval(config.refresh.draw());
    draw.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // Ctrl-C arrives as a keystroke while the terminal is in raw mode, but a
    // signal from elsewhere — a service manager, a `kill`, a closed ssh session
    // — would otherwise leave the terminal in raw mode on the alternate screen.
    let mut signals = Signals::new()?;

    let result = loop {
        // Draw first so the frame reflects everything applied below.
        if let Err(error) = terminal.draw(|frame| ui::draw(frame, &mut app, &theme)) {
            break Err(anyhow::Error::from(error));
        }

        tokio::select! {
            key = keys_rx.recv() => {
                let Some(key) = key else { break Ok(()) };
                match app.on_key(key) {
                    Action::Quit => break Ok(()),
                    Action::Refresh => {
                        let _ = controls_tx.try_send(Control::RefreshNow);
                    }
                    Action::SetPaused(paused) => {
                        let _ = controls_tx.try_send(Control::SetPaused(paused));
                    }
                    Action::Send(command) => {
                        let _ = controls_tx.try_send(Control::Send(command));
                    }
                    Action::None => {}
                }

                // Drain the rest of a key repeat before redrawing, so holding a
                // key scrolls smoothly instead of one frame at a time.
                while let Ok(key) = keys_rx.try_recv() {
                    match app.on_key(key) {
                        Action::Quit => break,
                        Action::Refresh => {
                            let _ = controls_tx.try_send(Control::RefreshNow);
                        }
                        Action::SetPaused(paused) => {
                            let _ = controls_tx.try_send(Control::SetPaused(paused));
                        }
                        Action::Send(command) => {
                            let _ = controls_tx.try_send(Control::Send(command));
                        }
                        Action::None => {}
                    }
                }
            }
            update = updates_rx.recv() => {
                let Some(update) = update else { break Ok(()) };
                app.apply(update);
                while let Ok(update) = updates_rx.try_recv() {
                    app.apply(update);
                }
            }
            _ = signals.recv() => break Ok(()),
            _ = draw.tick() => {}
        }
    };

    ratatui::try_restore()?;
    reader.stop();
    result
}

/// The signals that mean "stop", other than a keystroke.
struct Signals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    hangup: tokio::signal::unix::Signal,
}

impl Signals {
    fn new() -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                terminate: signal(SignalKind::terminate())?,
                hangup: signal(SignalKind::hangup())?,
            })
        }
        #[cfg(not(unix))]
        Ok(Self {})
    }

    async fn recv(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.terminate.recv() => {}
            _ = self.hangup.recv() => {}
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

/// A handle that tells the key-reading thread to stop.
struct Reader {
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl Reader {
    fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Read the terminal on a dedicated thread. Crossterm's reader blocks, and
/// blocking a runtime worker would stall the collectors behind it.
fn spawn_key_reader(sender: mpsc::Sender<Key>) -> Reader {
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = Arc::clone(&running);

    std::thread::spawn(move || {
        while flag.load(std::sync::atomic::Ordering::SeqCst) {
            match event::poll(POLL) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => return,
            }

            let Ok(event) = event::read() else { return };
            let key = match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => translate(key),
                // A wheel is a natural way to move a long region table.
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => Key::Down,
                    MouseEventKind::ScrollUp => Key::Up,
                    _ => continue,
                },
                // A resize needs no handling beyond the redraw it provokes.
                Event::Resize(..) => Key::Other,
                _ => continue,
            };

            if sender.blocking_send(key).is_err() {
                return;
            }
        }
    });

    Reader { running }
}

fn translate(key: KeyEvent) -> Key {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char(ch) if control => Key::Ctrl(ch.to_ascii_lowercase()),
        KeyCode::Char(ch) => Key::Char(ch),
        KeyCode::F(number) => Key::F(number),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        _ => Key::Other,
    }
}

/// Leave the terminal usable even when something panics mid-frame; otherwise
/// the shell is left in raw mode with no cursor.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = ratatui::try_restore();
        previous(info);
    }));
}
