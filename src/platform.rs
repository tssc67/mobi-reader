//! Native file drops enter the same import queue as the file picker.
use std::{
    collections::{HashMap, HashSet},
    io::ErrorKind,
    path::PathBuf,
};

use dioxus_core::EventHandler;
use dioxus_native::{
    use_window_event,
    winit::{
        data_transfer::{DataTransferId, TypeHint},
        event::WindowEvent,
        event_loop::{AsyncRequestSerial, DndAction},
    },
};

pub fn use_file_drop(on_paths: EventHandler<Vec<PathBuf>>, on_error: EventHandler<String>) {
    let mut accepted = HashSet::<DataTransferId>::new();
    let mut pending = HashMap::<DataTransferId, AsyncRequestSerial>::new();
    use_window_event(move |event, target| {
        match event {
            WindowEvent::DragEntered { id, .. } => match target.data_transfer(*id) {
                Ok(transfer) if transfer.has_type(&TypeHint::UriList) => {
                    match target.set_valid_dnd_actions(*id, &[DndAction::Copy]) {
                        Ok(()) => {
                            accepted.insert(*id);
                        }
                        Err(error) => {
                            on_error.call(format!("Cannot accept dropped files: {error}"))
                        }
                    }
                }
                Ok(_) => {
                    let _ = target.set_valid_dnd_actions(*id, &[]);
                }
                Err(error) => on_error.call(format!("Cannot inspect dropped files: {error}")),
            },
            WindowEvent::DragDropped { id, .. } if accepted.remove(id) => {
                match target.fetch_data_transfer(*id, &TypeHint::UriList) {
                    Ok(serial) => {
                        pending.insert(*id, serial);
                    }
                    Err(error) => on_error.call(format!("Cannot receive dropped files: {error}")),
                }
            }
            WindowEvent::DragLeft { id } => {
                accepted.remove(id);
            }
            WindowEvent::DataTransferReceived { id, serial, value }
                if pending.get(id) == Some(serial) =>
            {
                match value.try_as_file_paths() {
                    Ok(paths) => {
                        pending.remove(id);
                        if paths.is_empty() {
                            on_error.call("The drop did not contain any local files".into());
                        } else {
                            on_paths.call(paths);
                        }
                    }
                    // Winit may deliver another event once the asynchronous data is readable.
                    Err(error)
                        if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Deadlock) => {}
                    Err(error) => {
                        pending.remove(id);
                        on_error.call(format!(
                            "Drop EPUB or MOBI files from File Explorer: {error}"
                        ));
                    }
                }
            }
            _ => {}
        }
    });
}
