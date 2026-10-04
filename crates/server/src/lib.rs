use async_stream::stream;
use futures_core::stream::Stream;
use std::{ffi::c_void, pin::Pin, ptr::addr_of_mut};
use tokio::{
    io::{self, AsyncRead, AsyncWrite},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
};
use tonic::transport::server::Connected;
use windows::{
    core::w,
    Win32::Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION},
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    },
};

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
    pub fn new(path: &str) -> impl Stream<Item = io::Result<TonicNamedPipeServer>> {
        // set security attributes to allow ipc from sandboxed processes
        // see https://nathancorvussolis.blogspot.com/2018/05/windows-ime-security.html

        let name = format!("\\\\.\\pipe\\{}", path);

        let mut security_descriptor = PSECURITY_DESCRIPTOR::default();

        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:(A;;GA;;;AC)(A;;GA;;;RC)(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;BU)S:(ML;;NW;;;LW)"),
                SDDL_REVISION,
                &mut security_descriptor,
                None,
            )
            .unwrap();

            let mut security_attributes = UnsafeSecurityAttributes(SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: security_descriptor.0,
                bInheritHandle: false.into(),
            });

            stream! {
                let mut server = ServerOptions::new()
                    .first_pipe_instance(true)
                    .create_with_security_attributes_raw(
                        &name,
                        addr_of_mut!(security_attributes) as *mut c_void
                    )?;

                loop {
                    server.connect().await?;

                    let client = TonicNamedPipeServer {
                        inner: server,
                    };

                    yield Ok(client);

                    server = ServerOptions::new()
                        .create_with_security_attributes_raw(
                            &name,
                            addr_of_mut!(security_attributes) as *mut c_void
                        )?;
                }
            }
        }
    }
}


pub fn parse_magic_candidates(value: &serde_json::Value) -> Result<Vec<String>, String> {
    let choices = value
        .get("choices")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "APIレスポンスにchoicesがありません".to_string())?;

    let mut candidates = Vec::new();
    for choice in choices {
        let Some(content) = choice
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|value| value.as_str())
        else {
            continue;
        };

        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
            if let Some(predictions) = parsed.get("predictions").and_then(|value| value.as_array()) {
                for prediction in predictions {
                    if let Some(text) = prediction.as_str() {
                        let text = text.trim();
                        if !text.is_empty() && !candidates.iter().any(|item| item == text) {
                            candidates.push(text.to_string());
                        }
                    }
                }
                continue;
            }
        }

        let text = content.trim();
        if !text.is_empty() && !candidates.iter().any(|item| item == text) {
            candidates.push(text.to_string());
        }
    }

    candidates.truncate(5);
    if candidates.is_empty() {
        Err("APIレスポンスから候補を取得できませんでした".to_string())
    } else {
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_magic_candidates;

    #[test]
    fn parses_json_predictions_and_deduplicates() {
        let value = serde_json::json!({
            "choices": [
                {"message": {"content": "{\"predictions\":[\"候補A\",\"候補B\",\"候補A\"]}"}}
            ]
        });
        assert_eq!(
            parse_magic_candidates(&value).unwrap(),
            vec!["候補A".to_string(), "候補B".to_string()]
        );
    }

    #[test]
    fn accepts_plain_text_compatible_api_response() {
        let value = serde_json::json!({
            "choices": [
                {"message": {"content": "自然な候補"}}
            ]
        });
        assert_eq!(
            parse_magic_candidates(&value).unwrap(),
            vec!["自然な候補".to_string()]
        );
    }

    #[test]
    fn caps_candidates_at_five() {
        let value = serde_json::json!({
            "choices": [
                {"message": {"content": "{\"predictions\":[\"1\",\"2\",\"3\",\"4\",\"5\",\"6\"]}"}}
            ]
        });
        assert_eq!(parse_magic_candidates(&value).unwrap().len(), 5);
    }

    #[test]
    fn rejects_missing_choices() {
        assert!(parse_magic_candidates(&serde_json::json!({})).is_err());
    }
}
