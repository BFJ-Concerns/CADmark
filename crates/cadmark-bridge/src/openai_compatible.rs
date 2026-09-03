//! Shared transport for OpenAI Responses-compatible providers.

use std::fmt;
use std::time::Duration;

use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::backend::BackendError;

const MAX_PROVIDER_MESSAGE_CHARS: usize = 500;

pub(crate) struct Credential(String);

impl Credential {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    fn value(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Reusable provider client shared by every production AI consumer.
#[derive(Clone)]
pub struct OpenAiCompatibleClient {
    client: Client,
    responses_url: Url,
    model: String,
    credential: Option<std::sync::Arc<Credential>>,
}

impl OpenAiCompatibleClient {
    pub(crate) fn new(
        responses_url: Url,
        model: String,
        credential: Option<Credential>,
        timeout_seconds: u64,
    ) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(timeout_seconds))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent(concat!("CADmark/", env!("CARGO_PKG_VERSION")))
            .build()?;

        Ok(Self {
            client,
            responses_url,
            model,
            credential: credential.map(std::sync::Arc::new),
        })
    }

    pub(crate) async fn request_text(
        &self,
        instructions: &str,
        input: &str,
    ) -> Result<String, BackendError> {
        let body = ResponsesRequest {
            model: &self.model,
            instructions,
            input: vec![InputMessage {
                role: "user",
                content: vec![InputContent {
                    kind: "input_text",
                    text: input,
                }],
            }],
            store: false,
        };
        let mut request = self.client.post(self.responses_url.clone()).json(&body);
        if let Some(credential) = &self.credential {
            request = request.bearer_auth(credential.value());
        }

        let response = request.send().await.map_err(map_transport_error)?;
        let status = response.status();
        let response_body = response.text().await.map_err(map_transport_error)?;

        if !status.is_success() {
            return Err(non_success_error(
                status,
                &response_body,
                self.credential.as_deref(),
            ));
        }

        let envelope: ResponsesEnvelope = serde_json::from_str(&response_body)
            .map_err(|_| BackendError::ParseError("response was not valid JSON".to_string()))?;

        if let Some(error) = envelope.error {
            return Err(BackendError::RequestFailed(format_provider_error(
                None,
                &error,
                self.credential.as_deref(),
            )));
        }
        if matches!(envelope.status.as_deref(), Some("failed" | "incomplete")) {
            return Err(BackendError::RequestFailed(format!(
                "provider returned {} response state",
                envelope.status.unwrap_or_default()
            )));
        }

        extract_output_text(envelope.output)
    }
}

impl fmt::Debug for OpenAiCompatibleClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleClient")
            .field("responses_url", &"[CONFIGURED]")
            .field("model", &"[CONFIGURED]")
            .field("credential", &self.credential)
            .finish_non_exhaustive()
    }
}

/// Request body for the Responses API.
///
/// `input` is always the explicit message-array form rather than a bare
/// string: OpenAI accepts both, but Anthropic-translating gateways reject
/// the bare string with "cache_control cannot be set for empty text blocks".
#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    instructions: &'a str,
    input: Vec<InputMessage<'a>>,
    store: bool,
}

#[derive(Serialize)]
struct InputMessage<'a> {
    role: &'a str,
    content: Vec<InputContent<'a>>,
}

#[derive(Serialize)]
struct InputContent<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    text: &'a str,
}

#[derive(Deserialize)]
struct ResponsesEnvelope {
    output: Option<Vec<OutputItem>>,
    status: Option<String>,
    error: Option<ProviderError>,
}

#[derive(Deserialize)]
struct OutputItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    role: Option<String>,
    content: Option<Vec<ContentItem>>,
}

#[derive(Deserialize)]
struct ContentItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct ProviderErrorEnvelope {
    error: Option<ProviderError>,
}

#[derive(Deserialize)]
struct ProviderError {
    #[serde(rename = "type")]
    kind: Option<String>,
    code: Option<String>,
    message: Option<String>,
}

fn extract_output_text(output: Option<Vec<OutputItem>>) -> Result<String, BackendError> {
    let output =
        output.ok_or_else(|| BackendError::ParseError("response has no output".to_string()))?;
    let mut text = String::new();

    for item in output {
        if item.kind.as_deref() != Some("message") || item.role.as_deref() != Some("assistant") {
            continue;
        }
        for content in item.content.unwrap_or_default() {
            if content.kind.as_deref() == Some("output_text")
                && let Some(fragment) = content.text
            {
                text.push_str(&fragment);
            }
        }
    }

    if text.trim().is_empty() {
        Err(BackendError::ParseError(
            "response has no assistant output text".to_string(),
        ))
    } else {
        Ok(text)
    }
}

fn map_transport_error(error: reqwest::Error) -> BackendError {
    if error.is_timeout() {
        BackendError::Timeout
    } else if error.is_connect() {
        BackendError::Unavailable("could not connect to configured provider".to_string())
    } else {
        BackendError::RequestFailed("provider transport failed".to_string())
    }
}

fn non_success_error(
    status: StatusCode,
    body: &str,
    credential: Option<&Credential>,
) -> BackendError {
    let provider_error = serde_json::from_str::<ProviderErrorEnvelope>(body)
        .ok()
        .and_then(|envelope| envelope.error)
        .unwrap_or(ProviderError {
            kind: None,
            code: None,
            message: None,
        });
    BackendError::RequestFailed(format_provider_error(
        Some(status),
        &provider_error,
        credential,
    ))
}

fn format_provider_error(
    status: Option<StatusCode>,
    error: &ProviderError,
    credential: Option<&Credential>,
) -> String {
    let mut parts = Vec::new();
    if let Some(status) = status {
        parts.push(format!("HTTP {}", status.as_u16()));
    }
    if let Some(kind) = error.kind.as_deref().filter(|value| !value.is_empty()) {
        parts.push(format!("type {kind}"));
    }
    if let Some(code) = error.code.as_deref().filter(|value| !value.is_empty()) {
        parts.push(format!("code {code}"));
    }
    if let Some(message) = error.message.as_deref().filter(|value| !value.is_empty()) {
        let redacted = credential
            .map(|credential| message.replace(credential.value(), "[REDACTED]"))
            .unwrap_or_else(|| message.to_string());
        parts.push(truncate(&redacted, MAX_PROVIDER_MESSAGE_CHARS));
    }
    if parts.is_empty() {
        "provider request failed".to_string()
    } else {
        parts.join(": ")
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let shortened: String = characters.by_ref().take(max_chars).collect();
    if characters.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Debug)]
    struct RecordedRequest {
        path: String,
        authenticated: bool,
        body: serde_json::Value,
    }

    async fn recording_server(
        status: u16,
        body: String,
        delay: Option<Duration>,
    ) -> (
        Url,
        Arc<Mutex<Option<RecordedRequest>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let recorded = Arc::new(Mutex::new(None));
        let server_recorded = Arc::clone(&recorded);
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut chunk = [0_u8; 4096];
                let read = stream.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
                if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap();
            while bytes.len() < header_end + content_length {
                let mut chunk = [0_u8; 4096];
                let read = stream.read(&mut chunk).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
            }
            let request_line = headers.lines().next().unwrap();
            let path = request_line.split_whitespace().nth(1).unwrap().to_string();
            let authenticated = headers.lines().any(|line| {
                line.split_once(':').is_some_and(|(name, value)| {
                    name.eq_ignore_ascii_case("authorization")
                        && value
                            .trim()
                            .strip_prefix("Bearer ")
                            .is_some_and(|token| !token.is_empty())
                })
            });
            let request_body =
                serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap();
            *server_recorded.lock().unwrap() = Some(RecordedRequest {
                path,
                authenticated,
                body: request_body,
            });

            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
        (
            Url::parse(&format!("http://{address}/v1/responses")).unwrap(),
            recorded,
            handle,
        )
    }

    fn completed(text: &str) -> String {
        serde_json::json!({
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": text}]
            }]
        })
        .to_string()
    }

    #[test]
    fn extracts_ordered_assistant_text_and_ignores_reasoning() {
        let output = vec![
            OutputItem {
                kind: Some("reasoning".to_string()),
                role: None,
                content: None,
            },
            OutputItem {
                kind: Some("message".to_string()),
                role: Some("assistant".to_string()),
                content: Some(vec![
                    ContentItem {
                        kind: Some("output_text".to_string()),
                        text: Some("first".to_string()),
                    },
                    ContentItem {
                        kind: Some("output_text".to_string()),
                        text: Some(" second".to_string()),
                    },
                ]),
            },
        ];
        assert_eq!(extract_output_text(Some(output)).unwrap(), "first second");
    }

    #[test]
    fn rejects_missing_or_empty_output_text() {
        assert!(matches!(
            extract_output_text(None),
            Err(BackendError::ParseError(_))
        ));
        assert!(matches!(
            extract_output_text(Some(Vec::new())),
            Err(BackendError::ParseError(_))
        ));
    }

    #[test]
    fn provider_error_redacts_before_bounding() {
        let credential = Credential::new("fake-secret".to_string());
        let error = ProviderError {
            kind: Some("authentication_error".to_string()),
            code: Some("invalid_key".to_string()),
            message: Some(format!("bad fake-secret {}", "x".repeat(600))),
        };
        let formatted =
            format_provider_error(Some(StatusCode::UNAUTHORIZED), &error, Some(&credential));

        assert!(!formatted.contains("fake-secret"));
        assert!(formatted.contains("[REDACTED]"));
        assert!(formatted.chars().count() < 600);
    }

    #[tokio::test]
    async fn sends_exact_responses_contract_and_extracts_text() {
        let (url, recorded, server) = recording_server(200, completed("distinctive"), None).await;
        let client = OpenAiCompatibleClient::new(
            url,
            "configured-model".to_string(),
            Some(Credential::new("fake-token".to_string())),
            5,
        )
        .unwrap();

        let text = client
            .request_text("system content", "user content")
            .await
            .unwrap();
        server.await.unwrap();
        let request = recorded.lock().unwrap().take().unwrap();

        assert_eq!(text, "distinctive");
        assert_eq!(request.path, "/v1/responses");
        assert!(request.authenticated);
        assert_eq!(request.body["model"], "configured-model");
        assert_eq!(request.body["instructions"], "system content");
        assert_eq!(
            request.body["input"],
            serde_json::json!([{
                "role": "user",
                "content": [{"type": "input_text", "text": "user content"}]
            }])
        );
        assert_eq!(request.body["store"], false);
        assert_eq!(request.body.as_object().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn maps_malformed_and_structured_provider_failures_safely() {
        let (url, _, malformed_server) = recording_server(200, "{not-json".to_string(), None).await;
        let client = OpenAiCompatibleClient::new(url, "model".to_string(), None, 5).unwrap();
        assert!(matches!(
            client.request_text("instructions", "input").await,
            Err(BackendError::ParseError(_))
        ));
        malformed_server.await.unwrap();

        let secret = "fake-provider-secret";
        let provider_body = serde_json::json!({
            "error": {
                "type": "authentication_error",
                "code": "invalid_key",
                "message": format!("rejected {secret}")
            }
        })
        .to_string();
        let (url, _, provider_server) = recording_server(401, provider_body, None).await;
        let client = OpenAiCompatibleClient::new(
            url,
            "model".to_string(),
            Some(Credential::new(secret.to_string())),
            5,
        )
        .unwrap();
        let error = client
            .request_text("instructions", "input")
            .await
            .unwrap_err();
        provider_server.await.unwrap();
        assert!(matches!(error, BackendError::RequestFailed(_)));
        assert!(!error.to_string().contains(secret));
        assert!(error.to_string().contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn maps_unreachable_endpoint_and_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = OpenAiCompatibleClient::new(
            Url::parse(&format!("http://{address}/responses")).unwrap(),
            "model".to_string(),
            None,
            1,
        )
        .unwrap();
        assert!(matches!(
            client.request_text("instructions", "input").await,
            Err(BackendError::Unavailable(_))
        ));

        let (url, _, stalled_server) =
            recording_server(200, completed("late"), Some(Duration::from_secs(2))).await;
        let client = OpenAiCompatibleClient::new(url, "model".to_string(), None, 1).unwrap();
        assert!(matches!(
            client.request_text("instructions", "input").await,
            Err(BackendError::Timeout)
        ));
        stalled_server.await.unwrap();
    }
}
