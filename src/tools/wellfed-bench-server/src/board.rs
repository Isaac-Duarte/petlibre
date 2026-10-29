//! The relay board the server holds for its life, shared between the job
//! that switches it and the live state that shows it.
//!
//! Every set and query goes through one lock, so the state's relays are the
//! board's last answers: nothing else can switch a relay while the server
//! holds the port, except the board's own front buttons, which the query
//! after every job picks up.

use std::sync::{Arc, Mutex, MutexGuard};

use wellfed_formats::bench::now;
use wellfed_formats::bench::progress::RelayState;
use wellfed_relay::{Board, Switch};

use crate::state::Store;

/// The board's channels, 1 to 4.
const CHANNELS: std::ops::RangeInclusive<u8> = 1..=4;

#[derive(Clone)]
pub(crate) struct SharedBoard {
    board: Arc<Mutex<Box<dyn Board>>>,
    store: Store,
}

impl SharedBoard {
    pub(crate) fn new(board: Box<dyn Board>, store: Store) -> Self {
        Self {
            board: Arc::new(Mutex::new(board)),
            store,
        }
    }

    /// Ask the board for all four channels.
    pub(crate) fn query(&self) -> wellfed_relay::Result<()> {
        let mut board = self.lock();
        for channel in CHANNELS {
            let on = board.get(channel)?;
            self.publish(channel, on);
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Box<dyn Board>> {
        // A job that panicked mid-command leaves a board whose state the
        // next query reads again.
        self.board
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn publish(&self, channel: u8, on: bool) {
        self.store.update(|state| {
            state
                .relays
                .insert(channel.to_string(), RelayState { on, at: now() });
        });
    }
}

impl Board for SharedBoard {
    fn set(&mut self, channel: u8, switch: Switch) -> wellfed_relay::Result<()> {
        self.lock().set(channel, switch)?;
        self.publish(channel, switch.is_on());
        Ok(())
    }

    fn get(&mut self, channel: u8) -> wellfed_relay::Result<bool> {
        let on = self.lock().get(channel)?;
        self.publish(channel, on);
        Ok(on)
    }
}
