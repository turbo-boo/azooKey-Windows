use async_stream::stream;
use futures_core::stream::Stream;
use std::{ffi::c_void, pin::Pin, ptr::addr_of_mut};
use tokio::{
    io::{self, AsyncRead, AsyncWrite},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
};
use tonic::transport::server::Connected;
use windows::{
    core::HSTRING,
    Win32::Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION},
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    },
};

pub mod presentation;

#[allow(dead_code)]
struct UnsafeSecurityAttributes(SECURITY_ATTRIBUTES);

unsafe impl Send for UnsafeSecurityAttributes {}
unsafe impl Sync for UnsafeSecurityAttributes {}

pub struct TonicNamedPipeServer {
    inner: NamedPipeServer,
}

impl Connected for TonicNamedPipeServer {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {
        ()
    }
}

impl AsyncRead for TonicNamedPipeServer {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
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
                });
            };
            eprintln!("{name}: the pipe listener failed: {error}");
            yield Err(error);
        })
    }
}
