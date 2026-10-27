//! The interactive map: pan and zoom vector tiles drawn with braille characters.

mod app;
mod loader;
mod message;
mod palette;
mod prompt;
mod regions;
mod saved_view;
mod services;
mod trip;
mod ui;

use std::io::{self, BufWriter, Stdout};
use std::path::Path;
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant};

use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream};
use crossterm::execute;
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use tmaps_core::config::{Config, Paths, Units};
use tmaps_core::http;
use tmaps_core::providers::photon::Photon;
use tmaps_core::providers::valhalla::Valhalla;
use tmaps_core::tiles::{DEFAULT_CACHE_BYTES, DiskCache, TileSource};
use tokio::sync::{Notify, mpsc};
use tokio::time::MissedTickBehavior;

use crate::app::App;
use crate::loader::Loader;
use crate::message::Message;
use crate::palette::Palette;
use crate::services::Services;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] tmaps_core::Error),
    #[error("terminal error: {0}")]
    Terminal(#[from] io::Error),
    #[error("cannot save the map view: {0}")]
    SaveView(io::Error),
    #[error("a background task panicked")]
    Panic,
}

/// Shows the map until the user quits, then saves the view for the next start. Places that
/// the user saves go into `config_file`.
///
/// # Errors
///
/// [`Error::Terminal`] if the terminal fails, [`Error::SaveView`] if the view cannot be saved
/// after the quit, and [`Error::Panic`] if a background task panics.
pub async fn run(config: &Config, config_file: &Path, paths: &Paths) -> Result<(), Error> {
    let cache = DiskCache::new(paths.cache_dir.join("tiles"), DEFAULT_CACHE_BYTES);
    let client = http::client()?;
    let source = TileSource::new(client.clone(), &config.endpoints.tilejson, cache);
    let view = saved_view::initial(&paths.state_dir, config.places.get("home").copied());
    let mut app = App::new(view).with_places(config.places.clone());
    let (sender, receiver) = mpsc::unbounded_channel();
    let loader = Loader::new(source, &config.lang, sender.clone());
    let services = Services::new(
        Photon::new(client.clone(), &config.endpoints.photon),
        Valhalla::new(client, &config.endpoints.valhalla),
        &config.lang,
        config_file.to_owned(),
        sender,
    );
    let palette = Palette::detect(config.theme);

    let panicked = Arc::new(Notify::new());
    install_panic_hook(Arc::clone(&panicked));
    let mut terminal = init_terminal().inspect_err(|_| ratatui::restore())?;
    let result = match execute!(io::stdout(), EnableMouseCapture) {
        Ok(()) => {
            let channels = Channels {
                loader,
                services,
                receiver,
                panicked: &panicked,
            };
            event_loop(&mut terminal, &mut app, channels, palette, config.units).await
        }
        Err(error) => Err(error.into()),
    };
    // The drop flushes the rest of the buffer while the alternate screen is still active.
    drop(terminal);
    let _ = execute!(io::stdout(), EndSynchronizedUpdate, DisableMouseCapture);
    ratatui::restore();
    result?;
    saved_view::save(&paths.state_dir, app.view()).map_err(Error::SaveView)
}

type Terminal = ratatui::Terminal<CrosstermBackend<BufWriter<Stdout>>>;

/// Does the setup of `ratatui::try_init`, but with a `BufWriter`: `stdout()` is a `LineWriter`
/// with a 1 KiB buffer, so a frame of a pan would go to the terminal in hundreds of writes.
fn init_terminal() -> io::Result<Terminal> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(BufWriter::new(io::stdout())))
}

/// After the restore the map cannot continue, also when tokio catches the panic of a task.
fn install_panic_hook(panicked: Arc<Notify>) {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), EndSynchronizedUpdate, DisableMouseCapture);
        ratatui::restore();
        hook(info);
        panicked.notify_one();
    }));
}

/// 10 frames per second: the spinner moves, and an idle app does not draw.
const SPINNER_FRAME: Duration = Duration::from_millis(100);

/// What the event loop waits on, besides terminal events.
struct Channels<'a> {
    loader: Loader,
    services: Services,
    receiver: mpsc::UnboundedReceiver<Message>,
    panicked: &'a Notify,
}

async fn event_loop(
    terminal: &mut Terminal,
    app: &mut App,
    channels: Channels<'_>,
    palette: Palette,
    units: Units,
) -> Result<(), Error> {
    let Channels {
        loader,
        services,
        mut receiver,
        panicked,
    } = channels;
    let mut events = EventStream::new();
    let mut spinner = tokio::time::interval(SPINNER_FRAME);
    spinner.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;
    loop {
        let size = terminal.size()?;
        let (width, height) = ui::map_size(size.width, size.height, app.panel_visible());
        app.set_map_size(width, height);
        app.set_help_overflow(ui::help_overflow(size.width, size.height));
        let now = Instant::now();
        dirty |= app.expire(now);
        if app.wants_source(now) {
            loader.load_source();
        }
        loader.set_wanted(app.visible_tiles());
        for id in app.tiles_to_load(now) {
            loader.load(id);
        }
        for request in app.take_requests() {
            services.send(request);
        }
        if dirty {
            // The terminal shows the frame only when it is complete (DEC mode 2026). Terminals
            // without the mode ignore it. `execute!` sends the start now, so that the buffer is
            // empty if the draw panics: the drop must not send it after the restore.
            execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
            let mut regions = None;
            terminal.draw(|frame| regions = Some(ui::draw(frame, app, palette, units)))?;
            execute!(terminal.backend_mut(), EndSynchronizedUpdate)?;
            app.set_regions(regions.unwrap_or_default());
        }
        let deadline = app.next_deadline();
        dirty = tokio::select! {
            biased;
            () = panicked.notified() => return Err(Error::Panic),
            mut event = events.next() => {
                // A drag or a scroll queues many mouse events, and one frame shows them all. A
                // key or a resize can change the layout, so the next event needs a new frame.
                let mut dirty = false;
                loop {
                    let mouse = matches!(event, Some(Ok(Event::Mouse(_))));
                    match event {
                        Some(Ok(event)) => dirty |= app.handle(&event),
                        Some(Err(error)) => return Err(error.into()),
                        None => return Ok(()),
                    }
                    if !mouse || app.should_quit() {
                        break dirty;
                    }
                    // `poll!`, not `now_or_never`: `EventStream` keeps the waker of its first
                    // pending poll, and a no-op waker would lose the wakeup of the next input.
                    match futures_util::poll!(events.next()) {
                        Poll::Ready(next) => event = next,
                        Poll::Pending => break dirty,
                    }
                }
            }
            Some(message) = receiver.recv() => {
                let now = Instant::now();
                app.receive(message, now);
                while let Ok(message) = receiver.try_recv() {
                    app.receive(message, now);
                }
                true
            }
            _ = spinner.tick(), if app.busy() => {
                app.tick();
                true
            }
            () = tokio::time::sleep_until(deadline.unwrap_or(now).into()), if deadline.is_some() => true,
        };
        if app.should_quit() {
            return Ok(());
        }
    }
}
