use async_stream::stream;
use futures_core::stream::Stream;
use std::{
    ffi::c_void,
    os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
    pin::Pin,
    ptr::addr_of_mut,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
};
use tokio::{
    io::{self, AsyncRead, AsyncWrite},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
};
use tonic::transport::server::Connected;
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::HANDLE,
        Security::{
            Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION},
            RevertToSelf, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        },
        System::{
            Pipes::ImpersonateNamedPipeClient,
            Threading::{GetCurrentThread, OpenThreadToken},
        },
    },
};

pub mod presentation;

#[allow(dead_code)]
struct UnsafeSecurityAttributes(SECURITY_ATTRIBUTES);

unsafe impl Send for UnsafeSecurityAttributes {}
unsafe impl Sync for UnsafeSecurityAttributes {}

pub struct TonicNamedPipeServer {
    inner: NamedPipeServer,
    info: PipeConnectInfo,
}

/// Identifies the client of a pipe connection. tonic hands it to every
/// request of the connection in the request extensions.
#[derive(Debug, Clone)]
pub struct PipeConnectInfo {
    connection: u64,
    integrity: Arc<OnceLock<u32>>,
}

impl PipeConnectInfo {
    fn new() -> Self {
        // 0 is never handed out, so it can stand for "no connection"
        static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);
        Self {
            connection: NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed),
            integrity: Arc::default(),
        }
    }

    /// Distinguishes this connection from every other connection of this
    /// process; never 0.
    pub fn connection(&self) -> u64 {
        self.connection
    }

    /// The client's mandatory integrity level, comparable with
    /// [`shared::pipe::MEDIUM_INTEGRITY`]. Taken from the client's token
    /// when its first data arrives, so it is known by the time a request is
    /// handled; 0 (untrusted) when it could not be determined.
    pub fn integrity(&self) -> u32 {
        self.integrity.get().copied().unwrap_or(0)
    }
}

impl Connected for TonicNamedPipeServer {
    type ConnectInfo = PipeConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.info.clone()
    }
}

/// Integrity level of the token the client opened `pipe` with.
///
/// The client's process ID cannot be trusted for this (another process can
/// open the pipe and hand the handle over), so the client is impersonated.
/// Clients open the pipe at identification level, which allows querying
/// their token but not acting as them. Impersonation needs a message read
/// from the pipe when the client tracks its context dynamically.
fn client_integrity(pipe: &NamedPipeServer) -> io::Result<u32> {
    unsafe {
        ImpersonateNamedPipeClient(HANDLE(pipe.as_raw_handle()))?;
        let mut token = HANDLE::default();
        // as self: an identification-level token cannot open objects
        let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token);
        if RevertToSelf().is_err() {
            // The thread would go on running as the client.
            std::process::abort();
        }
        opened?;
        let token = OwnedHandle::from_raw_handle(token.0);
        shared::pipe::token_integrity(HANDLE(token.as_raw_handle()))
    }
}

impl AsyncRead for TonicNamedPipeServer {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if buf.filled().len() > before && self.info.integrity.get().is_none() {
            let integrity = client_integrity(&self.inner).unwrap_or_else(|error| {
                eprintln!("cannot identify the pipe client: {error}");
                0
            });
            let _ = self.info.integrity.set(integrity);
        }
        result
    }
}

impl AsyncWrite for TonicNamedPipeServer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl TonicNamedPipeServer {
    /// Serves the pipe [`shared::pipe::path`]`(base)` of the current user and
    /// session.
    ///
    /// Fails when the pipe already exists (another instance of the server
    /// runs in this session). The stream ends after its first error, which
    /// is also printed: tonic skips failed accepts and returns `Ok` when the
    /// stream ends, so callers must treat the end of serving as fatal.
    pub fn new(base: &str) -> io::Result<impl Stream<Item = io::Result<TonicNamedPipeServer>>> {
        // Allow IPC from sandboxed processes (AppContainer / restricted tokens),
        // see https://nathancorvussolis.blogspot.com/2018/05/windows-ime-security.html
        // Only SYSTEM, Administrators and the pipe owner get GENERIC_ALL. The
        // user running the server, and sandboxes running as that user, get
        // read/write data only: FILE_CREATE_PIPE_INSTANCE would let them
        // register a rogue instance of this pipe and intercept the IME's
        // traffic. Other users get nothing; for AppContainer and restricted
        // tokens both the user and the AC/RC entry must match.

        let name = shared::pipe::path(base)?;
        let user = shared::pipe::current_user_sid()?;
        let client = shared::pipe::CLIENT_ACCESS_SDDL;
        let sddl = HSTRING::from(format!(
            "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)(A;;{client};;;{user})(A;;{client};;;AC)(A;;{client};;;RC)S:(ML;;NW;;;LW)"
        ));

        let mut security_descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION,
                &mut security_descriptor,
                None,
            )?;
        }

        let mut security_attributes = UnsafeSecurityAttributes(SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security_descriptor.0,
            bInheritHandle: false.into(),
        });

        let mut server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .create_with_security_attributes_raw(
                    &name,
                    addr_of_mut!(security_attributes) as *mut c_void,
                )?
        };

        Ok(stream! {
            let error = loop {
                if let Err(error) = server.connect().await {
                    break error;
                }

                // Create the next instance before handing this one over:
                // once the connection is closed there would otherwise be
                // no instance left, and a client opening the pipe then
                // gets "not found" instead of ERROR_PIPE_BUSY.
                let connected = server;
                server = match unsafe {
                    ServerOptions::new().create_with_security_attributes_raw(
                        &name,
                        addr_of_mut!(security_attributes) as *mut c_void,
                    )
                } {
                    Ok(next) => next,
                    Err(error) => break error,
                };

                yield Ok(TonicNamedPipeServer {
                    inner: connected,
                    info: PipeConnectInfo::new(),
                });
            };
            eprintln!("{name}: the pipe listener failed: {error}");
            yield Err(error);
        })
    }
}
