mod app;
mod cache;
mod cli;
mod compat;
mod event;
mod package;
mod pm;
mod project;
mod registry;
mod self_update;
mod ui;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::Event;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::{
    app::{App, Msg, ProjectInfo},
    cli::Cli,
    event::EventReader,
};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.clear_cache {
        return cache::clear();
    }
    let project = ProjectInfo::load(&cli.cwd, cli.pm)?;
    let (mut app, rx) = App::new(project);
    app.start();

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, rx).await;
    ratatui::restore();
    result?;

    if let Some(version) = app.install_update {
        self_update::install(&version)?;
    }
    Ok(())
}

/// Draws and reacts to input and background results until the app quits.
async fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    mut messages: mpsc::UnboundedReceiver<Msg>,
) -> Result<()> {
    let mut events = EventReader::spawn();
    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, app))?;
        tokio::select! {
            Some(msg) = messages.recv() => app.handle_message(msg),
            Some(event) = events.next() => {
                if let Event::Key(key) = event {
                    app.handle_key(key)?;
                }
            }
        }
    }
    events
        .shutdown()
        .await
        .context("Terminal event reader task failed")
}
