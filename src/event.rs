//! Forwards terminal input from a blocking reader thread to the async event loop.

use std::time::Duration;

use crossterm::event::{self, Event};
use tokio::{sync::mpsc, task::JoinHandle};

pub struct EventReader {
    rx: mpsc::UnboundedReceiver<Event>,
    task: JoinHandle<()>,
}

impl EventReader {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        // Polling with a timeout lets the thread notice when the receiver is dropped.
        let task = tokio::task::spawn_blocking(move || {
            while !tx.is_closed() {
                match event::poll(Duration::from_millis(100)) {
                    Ok(true) => match event::read() {
                        Ok(event) => {
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    },
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
        });
        Self { rx, task }
    }

    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }

    /// Stops the reader thread and waits for it to exit.
    pub async fn shutdown(self) -> Result<(), tokio::task::JoinError> {
        drop(self.rx);
        self.task.await
    }
}
