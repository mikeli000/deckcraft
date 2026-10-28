//! "Upload Live": publishes an [`crate::export::export_pptx_for_live_upload`]-exported `.pptx`
//! into a real Seismic tenant's Content Library, the same way a human operating the tenant's own
//! REST surface by hand would: an OAuth client-credentials token, a blob upload of the raw file
//! bytes, a Content Manager call that turns that blob into a Library item of Seismic's "LiveDoc"
//! content type, and a best-effort call asking FormService to attach a default form to the new
//! version. [`upload_live_pptx`] runs exactly those four calls in order and reports what each one
//! produced.
//!
//! Every credential and base URL this needs comes from an environment variable
//! ([`SeismicCredentials::from_env`]) — never hardcoded, and the four base URLs are independently
//! override-able since a tenant's gateway routing can differ by environment.
//!
//! This module is desktop-only (see this crate's `Cargo.toml`): it blocks the calling thread for
//! the full round trip of all four calls, which would freeze a wasm32 UI thread; the caller
//! (`deckcraft-ui-egui`) runs it on a background thread instead.
//!
//! **Verified against a real tenant** (`real_upload_live_pptx_against_a_real_tenant`, `#[ignore]`d
//! since it needs real credentials): the token/blob/content-creation steps succeed against a real
//! Seismic tenant — a real content item really gets created, the part that actually matters.
//! The fourth (best-effort) step still fails regardless of which export function supplies
//! `pptx_bytes`; see `export::export_pptx_for_live_upload`'s own doc comment for what's been
//! ruled out and what's still unknown.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;

/// The tenant, identity and routing a live upload needs. The four `*_base` fields point at a QA
/// deployment by default (ordinary routing, not a secret) and can each be overridden
/// independently via their own environment variable, since a given tenant's gateways don't
/// necessarily all move together. Every other field is tenant- or identity-specific with no sane
/// default, so [`SeismicCredentials::from_env`] refuses to produce a half-configured value.
pub struct SeismicCredentials {
    pub tenant: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: String,
    pub auth_base: String,
    pub bss_base: String,
    pub bss_owner: String,
    pub cm_base: String,
    pub team_site_id: String,
    pub parent_folder_id: String,
    pub user_id: String,
    pub user_name: String,
    /// `https://{tenant}.seismic.com` unless overridden — a tenant's FormService endpoint can live
    /// on a different host than its CM/BSS gateways.
    pub form_service_base: String,
}

/// One environment variable [`upload_live_pptx`] can't run without, and didn't have — kept
/// distinct from [`UploadLiveError`] so a caller can separate "this feature was never set up
/// here" from "it's set up, but the upload itself went wrong".
#[derive(Debug)]
pub struct MissingCredential(pub &'static str);

impl std::fmt::Display for MissingCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "missing required environment variable {}", self.0)
    }
}

impl std::error::Error for MissingCredential {}

impl SeismicCredentials {
    pub fn from_env() -> Result<Self, MissingCredential> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// The real logic behind [`Self::from_env`], parameterized over the variable lookup so a test
    /// can supply an in-memory map instead of mutating the real process environment —
    /// `std::env::set_var`/`remove_var` need `unsafe` under Rust 2024, which this workspace
    /// forbids even in test code (see `AGENTS.md`'s "never crash" rule).
    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, MissingCredential> {
        let required = |name: &'static str| get(name).ok_or(MissingCredential(name));
        let optional = |name: &str, default: &str| get(name).unwrap_or_else(|| default.to_string());

        let tenant = required("SEISMIC_TENANT")?;
        let form_service_default = format!("https://{tenant}.seismic.com");
        Ok(SeismicCredentials {
            client_id: required("SEISMIC_CLIENT_ID")?,
            client_secret: required("SEISMIC_CLIENT_SECRET")?,
            scope: optional("SEISMIC_SCOPE", "library upload"),
            auth_base: required("SEISMIC_AUTH_BASE")?,
            bss_base: required("SEISMIC_BSS_BASE")?,
            bss_owner: optional("SEISMIC_BSS_OWNER", "ccp"),
            cm_base: required("SEISMIC_CM_BASE")?,
            team_site_id: optional("SEISMIC_TEAM_SITE_ID", "1"),
            parent_folder_id: required("SEISMIC_PARENT_FOLDER_ID")?,
            user_id: required("SEISMIC_USER_ID")?,
            user_name: required("SEISMIC_USER_NAME")?,
            form_service_base: optional("SEISMIC_FORM_SERVICE_BASE", &form_service_default),
            tenant,
        })
    }
}

/// What [`upload_live_pptx`] has to show for a run that got the content itself created. The last
/// two fields track the best-effort fourth call separately: a real Library item can exist validly
/// with no form attached yet, so a failure there is reported alongside a real `content_id`/
/// `library_url`, never instead of them.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UploadLiveResult {
    pub content_id: String,
    pub content_version_id: String,
    pub library_url: String,
    pub form_id: Option<String>,
    pub form_warning: Option<String>,
}

/// Why one of the three *fatal* calls (token, blob, content — not the form attempt, which never
/// produces this) didn't produce a usable result.
#[derive(Debug)]
pub enum UploadLiveError {
    Http(reqwest::Error),
    Api { step: &'static str, status: u16, body: String },
}

impl std::fmt::Display for UploadLiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadLiveError::Http(err) => write!(f, "network error talking to Seismic: {err}"),
            UploadLiveError::Api { step, status, body } => write!(f, "Seismic API call ({step}) failed with status {status}: {body}"),
        }
    }
}

impl std::error::Error for UploadLiveError {}

impl From<reqwest::Error> for UploadLiveError {
    fn from(err: reqwest::Error) -> Self {
        UploadLiveError::Http(err)
    }
}

/// Runs the full four-call sequence against one tenant and reports what came back. Blocking: this
/// runs on whatever thread calls it, so the caller is expected to use a background thread (see
/// this module's own doc comment).
///
/// `pptx_bytes` should come from [`crate::export::export_pptx_for_live_upload`] — a plain
/// `deckcraft_pptx::export` would still create *a* Library item, just not one the tenant's own
/// LiveDocs add-in would recognize as a template.
///
/// The fourth call is never fatal: a real tenant can hold perfectly valid content with no default
/// form attached, and by the time that call runs the content itself (steps one through three) is
/// already real in the Library. Its outcome always lands in `form_id`/`form_warning`, never in
/// `Err`.
pub fn upload_live_pptx(
    creds: &SeismicCredentials,
    pptx_bytes: Vec<u8>,
    filename: &str,
    content_name: &str,
) -> Result<UploadLiveResult, UploadLiveError> {
    let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(60)).build()?;

    let access_token = request_access_token(&client, creds)?;
    let blob_id = upload_blob(&client, creds, &access_token, pptx_bytes, filename)?;
    let content = create_library_content(&client, creds, &access_token, &blob_id, content_name)?;
    let (form_id, form_warning) = try_attach_default_form(&client, creds, &access_token, &content.version_id);

    Ok(UploadLiveResult {
        library_url: library_url(creds, &content.id),
        content_id: content.id,
        content_version_id: content.version_id,
        form_id,
        form_warning,
    })
}

/// Step 1: an OAuth `client_credentials` grant against the tenant's auth gateway.
fn request_access_token(client: &reqwest::blocking::Client, creds: &SeismicCredentials) -> Result<String, UploadLiveError> {
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
    }
    let url = format!("{}/tenants/{}/connect/token", creds.auth_base, creds.tenant);
    let resp = client
        .post(&url)
        .form(&[
            ("client_id", creds.client_id.as_str()),
            ("client_secret", creds.client_secret.as_str()),
            ("grant_type", "client_credentials"),
            ("scope", creds.scope.as_str()),
        ])
        .send()?;
    Ok(read_json::<TokenResponse>("get token", resp)?.access_token)
}

/// Step 2: BSS stores the raw file bytes as a blob and hands back an id the content-creation call
/// (step 3) points at.
fn upload_blob(
    client: &reqwest::blocking::Client,
    creds: &SeismicCredentials,
    access_token: &str,
    pptx_bytes: Vec<u8>,
    filename: &str,
) -> Result<String, UploadLiveError> {
    #[derive(Deserialize)]
    struct BlobResponse {
        id: String,
    }
    let metadata = serde_json::json!({ "format": "pptx", "expiration": "-1", "filename": filename }).to_string();
    let url = format!("{}/api/upload/v1/blobs?t={}", creds.bss_base, creds.tenant);
    let resp = client
        .post(&url)
        .bearer_auth(access_token)
        .header("Content-Type", "application/octet-stream")
        .header("x-seismic-blob-metadata", metadata)
        .header("x-seismic-blob-owner", &creds.bss_owner)
        .header("x-client-application-name", "deckcraft")
        .header("x-client-application-version", env!("CARGO_PKG_VERSION"))
        .body(pptx_bytes)
        .send()?;
    Ok(read_json::<BlobResponse>("upload blob", resp)?.id)
}

/// Step 3: Content Manager turns the uploaded blob into a Library item with content type `1`
/// (Seismic's "LiveDoc" type). Its own response is PascalCase, unlike every other call here.
struct CreatedContent {
    id: String,
    version_id: String,
}

fn create_library_content(
    client: &reqwest::blocking::Client,
    creds: &SeismicCredentials,
    access_token: &str,
    blob_id: &str,
    content_name: &str,
) -> Result<CreatedContent, UploadLiveError> {
    #[derive(Deserialize)]
    struct ContentResponse {
        #[serde(rename = "Id")]
        id: String,
        #[serde(rename = "VersionId")]
        version_id: String,
    }
    let url = format!("{}/v1.0/tenants/{}/unified/teamSites/{}/files", creds.cm_base, creds.tenant, creds.team_site_id);
    let body = serde_json::json!({
        "fileName": content_name,
        "format": "pptx",
        "contentType": 1,
        "uploadedBlobId": blob_id,
        "parentFolderId": creds.parent_folder_id,
        "isAutoRename": true,
        "description": "Uploaded live by DeckCraft",
    });
    let resp = client.post(&url).bearer_auth(access_token).header("X-Seismic-UserId", &creds.user_id).json(&body).send()?;
    let parsed = read_json::<ContentResponse>("create content", resp)?;
    Ok(CreatedContent { id: parsed.id, version_id: parsed.version_id })
}

/// Step 4 (best-effort — see [`upload_live_pptx`]'s doc comment): asks FormService to attach a
/// default form to the content version just created. Every way this can fail to produce a form id
/// ends up as `(None, Some(reason))`, never a propagated error.
fn try_attach_default_form(
    client: &reqwest::blocking::Client,
    creds: &SeismicCredentials,
    access_token: &str,
    content_version_id: &str,
) -> (Option<String>, Option<String>) {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let url = format!("{}/JsonServiceApi/FormService.svc/CreateNewForm", creds.form_service_base);
    let sent = client
        .get(&url)
        .bearer_auth(access_token)
        .query(&[
            ("tenant", creds.tenant.as_str()),
            ("user", creds.user_name.as_str()),
            ("timestamp", ts.to_string().as_str()),
            ("signature", "x"),
            ("cvid", content_version_id),
            ("name", "Default"),
        ])
        .send();

    let resp = match sent {
        Ok(resp) => resp,
        Err(err) => return (None, Some(format!("FormService request failed: {err}"))),
    };
    if !resp.status().is_success() {
        return (None, Some(format!("FormService returned status {}", resp.status())));
    }
    let body: Value = match resp.json() {
        Ok(v) => v,
        Err(err) => return (None, Some(format!("couldn't parse FormService response: {err}"))),
    };
    form_service_outcome(&body)
}

/// Picks apart a FormService JSON body: `"Error"` is an empty string on success, or either a
/// non-empty string or an object (`{"Name": ..., "Message": ...}`) describing a failure — this
/// endpoint doesn't settle on one shape for both outcomes. `"ServiceResult"."FormId"` is the id
/// this call exists to get, when there's no error to report instead.
fn form_service_outcome(body: &Value) -> (Option<String>, Option<String>) {
    let form_id = || body.get("ServiceResult").and_then(|r| r.get("FormId")).and_then(Value::as_str).map(str::to_string);
    match body.get("Error") {
        None | Some(Value::Null) => (form_id(), None),
        Some(Value::String(s)) if s.is_empty() => (form_id(), None),
        Some(Value::String(s)) => (None, Some(s.clone())),
        Some(err @ Value::Object(_)) => {
            let name = err.get("Name").and_then(Value::as_str).filter(|n| !n.is_empty());
            let message = err.get("Message").and_then(Value::as_str).map(decode_form_urlencoded).filter(|m| !m.is_empty());
            let detail = match (name, message) {
                (Some(name), Some(message)) => format!("{name}: {message}"),
                (Some(name), None) => name.to_string(),
                (None, Some(message)) => message,
                (None, None) => "FormService returned an error with no message".to_string(),
            };
            (None, Some(detail))
        }
        Some(_) => (None, Some("FormService's \"Error\" field had an unexpected shape".to_string())),
    }
}

fn library_url(creds: &SeismicCredentials, content_id: &str) -> String {
    format!("https://{}.seismic.com/apps/contentmanager/detail/{}/{}/info/LIST/title", creds.tenant, creds.team_site_id, content_id)
}

/// Shared handling for the three calls above whose failure means the whole upload failed. Reads
/// the body once so both "non-2xx status" and "2xx but the body doesn't decode" end up with the
/// real response text attached, not just a bare status code.
fn read_json<T: for<'de> Deserialize<'de>>(step: &'static str, resp: reqwest::blocking::Response) -> Result<T, UploadLiveError> {
    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(UploadLiveError::Api { step, status: status.as_u16(), body });
    }
    serde_json::from_str(&body).map_err(|err| UploadLiveError::Api {
        step,
        status: status.as_u16(),
        body: format!("couldn't parse response ({err}): {body}"),
    })
}

/// FormService's own `Message` field comes back `application/x-www-form-urlencoded`-encoded
/// (spaces as `+`, everything else as `%XX`) rather than plain text. Walks the raw bytes directly
/// rather than slicing the original `&str`, so a truncated or non-ASCII `%XX` sequence can't panic
/// on a non-UTF8 boundary.
fn decode_form_urlencoded(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(decoded) => {
                        out.push(decoded);
                        i += 3;
                    }
                    None => {
                        out.push(b);
                        i += 1;
                    }
                }
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn from_lookup_fails_loudly_on_a_missing_required_variable_and_fills_in_sensible_defaults() {
        let base: std::collections::HashMap<&str, &str> = [
            ("SEISMIC_TENANT", "acme"),
            ("SEISMIC_CLIENT_ID", "id"),
            ("SEISMIC_CLIENT_SECRET", "secret"),
            ("SEISMIC_PARENT_FOLDER_ID", "folder"),
            ("SEISMIC_USER_ID", "uid"),
            ("SEISMIC_USER_NAME", "uname"),
            ("SEISMIC_AUTH_BASE", "https://auth.example.test"),
            ("SEISMIC_BSS_BASE", "https://bss.example.test"),
            ("SEISMIC_CM_BASE", "https://gateway.example.test/cm"),
        ]
        .into_iter()
        .collect();

        let creds = SeismicCredentials::from_lookup(|k| base.get(k).map(|v| v.to_string())).expect("all required vars are set");
        assert_eq!(creds.tenant, "acme");
        assert_eq!(creds.auth_base, "https://auth.example.test");
        assert_eq!(creds.scope, "library upload");
        assert_eq!(creds.form_service_base, "https://acme.seismic.com", "an unset SEISMIC_FORM_SERVICE_BASE should derive from the tenant");

        let mut missing_secret = base.clone();
        missing_secret.remove("SEISMIC_CLIENT_SECRET");
        match SeismicCredentials::from_lookup(|k| missing_secret.get(k).map(|v| v.to_string())) {
            Err(MissingCredential(name)) => assert_eq!(name, "SEISMIC_CLIENT_SECRET"),
            Ok(_) => panic!("should fail when a required variable is missing"),
        }
    }

    fn test_credentials(mock_base: &str) -> SeismicCredentials {
        SeismicCredentials {
            tenant: "acme".to_string(),
            client_id: "test-client".to_string(),
            client_secret: "test-secret".to_string(),
            scope: "library upload".to_string(),
            auth_base: mock_base.to_string(),
            bss_base: mock_base.to_string(),
            bss_owner: "ccp".to_string(),
            cm_base: mock_base.to_string(),
            team_site_id: "1".to_string(),
            parent_folder_id: "folder-123".to_string(),
            user_id: "user-123".to_string(),
            user_name: "mikeli".to_string(),
            form_service_base: mock_base.to_string(),
        }
    }

    async fn mount_happy_path(server: &MockServer, form_body: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/tenants/acme/connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "access_token": "tok-abc" })))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/upload/v1/blobs"))
            .and(query_param("t", "acme"))
            .and(header("authorization", "Bearer tok-abc"))
            .and(header("content-type", "application/octet-stream"))
            .and(header("x-seismic-blob-owner", "ccp"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({ "id": "blob-xyz" })))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1.0/tenants/acme/unified/teamSites/1/files"))
            .and(header("authorization", "Bearer tok-abc"))
            .and(header("x-seismic-userid", "user-123"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({ "Id": "content-1", "VersionId": "version-1" })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/JsonServiceApi/FormService.svc/CreateNewForm"))
            .and(query_param("cvid", "version-1"))
            .and(query_param("user", "mikeli"))
            .respond_with(ResponseTemplate::new(200).set_body_json(form_body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn upload_live_pptx_runs_all_four_calls_and_returns_their_ids() {
        let server = MockServer::start().await;
        let creds = test_credentials(&server.uri());
        mount_happy_path(&server, serde_json::json!({ "Error": "", "ServiceResult": { "FormId": "form-9" } })).await;

        let result = tokio::task::spawn_blocking(move || upload_live_pptx(&creds, b"fake pptx bytes".to_vec(), "test.pptx", "my-livedoc"))
            .await
            .unwrap()
            .expect("every mocked call succeeds, so this should too");

        assert_eq!(result.content_id, "content-1");
        assert_eq!(result.content_version_id, "version-1");
        assert_eq!(result.library_url, "https://acme.seismic.com/apps/contentmanager/detail/1/content-1/info/LIST/title");
        assert_eq!(result.form_id, Some("form-9".to_string()));
        assert_eq!(result.form_warning, None);
    }

    /// Content creation (step 3) succeeds even though FormService (step 4) fails outright — the
    /// best-effort step never turns a real upload into an overall failure.
    #[tokio::test]
    async fn a_failed_form_service_call_does_not_fail_the_whole_upload() {
        let server = MockServer::start().await;
        let creds = test_credentials(&server.uri());

        Mock::given(method("POST"))
            .and(path("/tenants/acme/connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "access_token": "tok-abc" })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/upload/v1/blobs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({ "id": "blob-xyz" })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1.0/tenants/acme/unified/teamSites/1/files"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({ "Id": "content-1", "VersionId": "version-1" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/JsonServiceApi/FormService.svc/CreateNewForm"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let result = tokio::task::spawn_blocking(move || upload_live_pptx(&creds, b"fake pptx bytes".to_vec(), "test.pptx", "my-livedoc"))
            .await
            .unwrap()
            .expect("a FormService failure should not fail the overall upload");

        assert_eq!(result.content_id, "content-1");
        assert_eq!(result.content_version_id, "version-1");
        assert_eq!(result.form_id, None);
        assert!(result.form_warning.is_some(), "the caller should still learn the form step failed");
    }

    /// Same non-fatal contract, but with the real object-shaped `Error` payload a live tenant
    /// actually returns (HTTP 200, app-level failure inside the body) — this exercises
    /// [`form_service_outcome`]'s object branch and the urlencoded message decoder together.
    #[tokio::test]
    async fn a_form_service_app_level_error_surfaces_its_decoded_message() {
        let server = MockServer::start().await;
        let creds = test_credentials(&server.uri());
        mount_happy_path(
            &server,
            serde_json::json!({
                "Error": { "Name": "Exception", "Message": "Missing+metadata+xml+in+template+source%3aPresentation" },
                "ServiceResult": null
            }),
        )
        .await;

        let result = tokio::task::spawn_blocking(move || upload_live_pptx(&creds, b"fake pptx bytes".to_vec(), "test.pptx", "my-livedoc"))
            .await
            .unwrap()
            .expect("an app-level FormService error should not fail the overall upload");

        assert_eq!(result.form_id, None);
        assert_eq!(result.form_warning.as_deref(), Some("Exception: Missing metadata xml in template source:Presentation"));
    }

    /// A failure in one of the three fatal steps (here: the blob upload) does propagate as `Err`,
    /// carrying which step and the real status/body for debugging against a real tenant.
    #[tokio::test]
    async fn a_failed_blob_upload_fails_the_whole_call_with_a_readable_error() {
        let server = MockServer::start().await;
        let creds = test_credentials(&server.uri());

        Mock::given(method("POST"))
            .and(path("/tenants/acme/connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "access_token": "tok-abc" })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/upload/v1/blobs"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;

        let err = tokio::task::spawn_blocking(move || upload_live_pptx(&creds, b"fake pptx bytes".to_vec(), "test.pptx", "my-livedoc"))
            .await
            .unwrap()
            .expect_err("a 403 on the blob upload should fail the whole call");

        match err {
            UploadLiveError::Api { step, status, body } => {
                assert_eq!(step, "upload blob");
                assert_eq!(status, 403);
                assert!(body.contains("forbidden"));
            }
            other => panic!("expected UploadLiveError::Api, got {other:?}"),
        }
    }

    #[test]
    fn decode_form_urlencoded_handles_plus_percent_and_a_truncated_escape() {
        assert_eq!(decode_form_urlencoded("a+b"), "a b");
        assert_eq!(decode_form_urlencoded("source%3aPresentation"), "source:Presentation");
        assert_eq!(decode_form_urlencoded("trailing%2"), "trailing%2", "an incomplete escape at the end should pass through literally");
    }

    /// Not run in CI (`#[ignore]`): makes a REAL call against a real Seismic tenant, using
    /// whatever `.env` at the repo root provides (`cargo test` runs with the workspace root as
    /// its working directory, which `dotenvy::dotenv()` searches by default). Run explicitly with
    /// `cargo test -p deckcraft-livedoc --lib -- --ignored real_upload_live_pptx_against_a_real_tenant`
    /// after setting up `.env` (copy `.env.example` at the repo root and fill in real
    /// credentials). The only place this module's real-world behavior is actually confirmed.
    #[test]
    #[ignore = "makes a real network call against a real Seismic tenant; needs .env"]
    fn real_upload_live_pptx_against_a_real_tenant() {
        let _ = dotenvy::dotenv();
        let creds = SeismicCredentials::from_env().expect("set up .env at the repo root first (see .env.example)");

        let mut p = deckcraft_model::defaults::new_presentation(None);
        let shape = deckcraft_model::Shape {
            text: Some(deckcraft_model::text::TextBody::from_text(&crate::display_text("smoke_test_field"))),
            ..Default::default()
        };
        p.slides = vec![std::sync::Arc::new(deckcraft_model::Slide { shapes: vec![shape], ..Default::default() })];
        crate::recognize_variables(&mut p);
        let bytes = crate::export::export_pptx_for_live_upload(&p, &creds.tenant).expect("should export cleanly");

        let result = upload_live_pptx(&creds, bytes, "deckcraft-livedoc-smoke-test.pptx", "DeckCraft livedoc smoke test")
            .expect("the real upload should succeed");
        println!("Uploaded: {}", result.library_url);
        if let Some(w) = &result.form_warning {
            println!("(form step warning, not fatal: {w})");
        }
    }
}
