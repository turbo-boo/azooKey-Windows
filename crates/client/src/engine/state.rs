use std::sync::{LazyLock, Mutex, MutexGuard};

use super::{
    input_mode::InputMode,
    ipc_service::{converter_availability, Availability, IPCService},
};

#[derive(Debug)]
pub struct IMEState {
    pub ipc_service: Option<IPCService>,
    pub input_mode: InputMode,
}

pub static IME_STATE: LazyLock<Mutex<IMEState>> = LazyLock::new(|| {
    tracing::debug!("Creating IMEState");
    Mutex::new(IMEState {
        ipc_service: None,
        input_mode: InputMode::default(),
    })
});
unsafe impl Sync for IMEState {}
unsafe impl Send for IMEState {}

impl IMEState {
    pub fn get() -> anyhow::Result<MutexGuard<'static, IMEState>> {
        match IME_STATE.try_lock() {
            Ok(guard) => Ok(guard),
            Err(e) => anyhow::bail!("Failed to lock state: {:?}", e),
        }
    }

    /// Returns the server connection, connecting first if the server was not
    /// reachable when the text service was activated (e.g. right after logon).
    ///
    /// Fails while the converter is paused after a missed deadline, so keys
    /// go to the application as when no server runs; once the pause is over
    /// one caller probes it first.
    pub fn ipc_service() -> anyhow::Result<IPCService> {
        let stored = Self::get()?.ipc_service.clone();
        let probe = match converter_availability() {
            Availability::Available => false,
            Availability::ProbeDue => true,
            Availability::Paused => anyhow::bail!("the server is not answering; trying later"),
        };

        if let Some(mut ipc_service) = stored {
            if probe {
                ipc_service.probe()?;
            }
            return Ok(ipc_service);
        }

        // Connect without holding the lock: IMEState::get only tries to lock,
        // so other threads' keys and the language bar icon would fail while
        // this waits for the server.
        let mut ipc_service = IPCService::new()?;
        ipc_service.probe()?;

        // another thread may have connected meanwhile; keep one connection
        Ok(Self::get()?.ipc_service.get_or_insert(ipc_service).clone())
    }
}
