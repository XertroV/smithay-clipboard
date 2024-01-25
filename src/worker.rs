use std::io::{Error, ErrorKind, Result};
use std::sync::mpsc::Sender;

use sctk::reexports::calloop::channel::Channel;
use sctk::reexports::calloop::{channel, EventLoop};
use sctk::reexports::calloop_wayland_source::WaylandSource;
use sctk::reexports::client::globals::registry_queue_init;
use sctk::reexports::client::Connection;

use crate::state::{SelectionTarget, State};

/// Spawn a clipboard worker, which dispatches its own `EventQueue` and handles
/// clipboard requests.
pub fn spawn(
    name: String,
    display: Connection,
    rx_chan: Channel<Command>,
    worker_replier: Sender<Result<String>>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            worker_impl(display, rx_chan, worker_replier);
        })
        .ok()
}

/// Clipboard worker thread command.
#[derive(Eq, PartialEq)]
pub enum Command {
    /// Store data to a clipboard.
    Store(String),
    /// Store data to a primary selection.
    StorePrimary(String),
    /// Load data from a clipboard.
    Load,
    /// Load primary selection.
    LoadPrimary,
    /// Shutdown the worker.
    Exit,
}

/// Handle clipboard requests.
fn worker_impl(
    connection: Connection,
    rx_chan: Channel<Command>,
    reply_tx: Sender<Result<String>>,
) {
    let (globals, event_queue) = match registry_queue_init(&connection) {
        Ok(data) => data,
        Err(_) => return,
    };

    let mut event_loop = EventLoop::<State>::try_new().unwrap();
    let loop_handle = event_loop.handle();

    let mut state =
        match State::new(&globals, &event_queue.handle(), loop_handle.clone(), reply_tx.clone()) {
            Some(state) => state,
            None => return,
        };

    // 'reconnect: loop {
    //     let loop_handle = loop_handle.clone();

    loop_handle
        .insert_source(rx_chan, |event, _, state| {
            if let channel::Event::Msg(event) = event {
                match event {
                    Command::StorePrimary(contents) => {
                        state.store_selection(SelectionTarget::Primary, contents);
                    },
                    Command::Store(contents) => {
                        state.store_selection(SelectionTarget::Clipboard, contents);
                    },
                    Command::Load if state.data_device_manager_state.is_some() => {
                        if let Err(err) = state.load_selection(SelectionTarget::Clipboard) {
                            let _ = state.reply_tx.send(Err(err));
                        }
                    },
                    Command::LoadPrimary if state.data_device_manager_state.is_some() => {
                        if let Err(err) = state.load_selection(SelectionTarget::Primary) {
                            let _ = state.reply_tx.send(Err(err));
                        }
                    },
                    Command::Load | Command::LoadPrimary => {
                        let _ = state.reply_tx.send(Err(Error::new(
                            ErrorKind::Other,
                            "requested selection is not supported",
                        )));
                    },
                    Command::Exit => state.exit = true,
                }
            }
        })
        .unwrap();

    let insert_registration = WaylandSource::new(connection, event_queue).insert(loop_handle);
    println!("insert_registration: {:#?}", insert_registration);

    if let Err(e) = insert_registration {
        // #[cfg(feature = "debug")]
        let err_msg = format!("Failed to insert wayland source: {:#?}", e);
        println!("{}", err_msg);
        let _ = state.reply_tx.send(Err(e.error.into()));
        // return Err(Error::new(ErrorKind::Other, err_msg));
        return;
    }

    loop {
        let dispatch_resp = event_loop.dispatch(None, &mut state);
        println!("dispatch_resp: {:?}", dispatch_resp);
        if let Err(e) = dispatch_resp {
            // #[cfg(feature = "debug")]
            let err_msg = format!("Error while dispatching: {:#?}", e);
            println!("{}", err_msg);
            let _ = state.reply_tx.send(Err(e.into()));
            state.exit = true;
        }

        if state.exit {
            // break 'reconnect;
            break;
        }
    }
    // }
    println!("smithay-clipboard worker thread exited");
}
