use std::sync::{LazyLock, Mutex, MutexGuard};

use super::{input_mode::InputMode, ipc_service::IPCService};

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
    pub fn ipc_service() -> anyhow::Result<IPCService> {
        let mut state = Self::get()?;
        if let Some(ipc_service) = &state.ipc_service {
            return Ok(ipc_service.clone());
        }

        let mut ipc_service = IPCService::new()?;
        ipc_service.append_text("".to_string())?;
        state.ipc_service = Some(ipc_service.clone());
        Ok(ipc_service)
    }
}
