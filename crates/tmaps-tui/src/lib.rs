//! The interactive map: pan and zoom vector tiles drawn with braille characters.

mod app;
mod loader;
mod message;
mod palette;
mod prompt;
mod saved_view;
mod services;
mod trip;
mod ui;

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture, EventStream};
use crossterm::execute;
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tmaps_core::config::{Config, Paths, Units};
use tmaps_core::http;
use tmaps_core::providers::photon::Photon;
use tmaps_core::providers::valhalla::Valhalla;
use tmaps_core::tiles::{DEFAULT_CACHE_BYTES, DiskCache, TileSource};
use tokio::sync::{Notify, mpsc};

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

    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    let panicked = Arc::new(Notify::new());
    install_panic_hook(Arc::clone(&panicked));
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
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result?;
    saved_view::save(&paths.state_dir, app.view()).map_err(Error::SaveView)
}

/// `ratatui::try_init` restores the screen on a panic, but not the mouse mode. After the
/// restore the map cannot continue, also when tokio catches the panic of a task.
fn install_panic_hook(panicked: Arc<Notify>) {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        hook(info);
        panicked.notify_one();
    }));
}

/// What the event loop waits on, besides terminal events.
struct Channels<'a> {
    loader: Loader,
    services: Services,
    receiver: mpsc::UnboundedReceiver<Message>,
    panicked: &'a Notify,
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
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
    let mut dirty = true;
    loop {
        let size = terminal.size()?;
        let (width, height) = ui::map_size(size.width, size.height, app.panel_visible());
        app.set_map_size(width, height);
        let now = Instant::now();
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
            terminal.draw(|frame| ui::draw(frame, app, palette, units))?;
        }
        let retry = app.next_retry();
        dirty = tokio::select! {
            biased;
            () = panicked.notified() => return Err(Error::Panic),
            event = events.next() => match event {
                Some(Ok(event)) => app.handle(&event),
                Some(Err(error)) => return Err(error.into()),
                None => return Ok(()),
            },
            Some(message) = receiver.recv() => {
                let now = Instant::now();
                app.receive(message, now);
                while let Ok(message) = receiver.try_recv() {
                    app.receive(message, now);
                }
                true
            }
            () = tokio::time::sleep_until(retry.unwrap_or(now).into()), if retry.is_some() => true,
        };
        if app.should_quit() {
            return Ok(());
        }
    }
}
