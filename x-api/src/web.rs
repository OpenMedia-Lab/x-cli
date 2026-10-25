//! Internal web-API backend.
//!
//! [`WebBackend`] reuses the browser's logged-in session cookies instead of
//! developer credentials. It translates the paths the CLI already issues
//! (`/2/tweets`, `/2/media/upload`, `/2/tweets/{id}`) onto the endpoints the
//! x.com web app itself calls, so `x update` / `x reply` work unchanged.
//!
//! Authentication is cookie-based: `auth_token` plus `ct0` from a local
//! browser profile, the public web bearer token, and the CSRF header. No
//! official developer credentials are required.

use std::collections::HashMap;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, ORIGIN, REFERER};
use serde_json::{Map, Value};

use crate::backend::{AuthScheme, Backend, BackendError, CallRecord};

/// Public bearer token embedded in the x.com web client.
const WEB_BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const HOME_URL: &str = "https://x.com/home";
const GRAPHQL_BASE: &str = "https://x.com/i/api/graphql";
const MEDIA_UPLOAD_URL: &str = "https://upload.x.com/i/media/upload.json";
const MAIN_JS_HOST: &str = "https://abs.twimg.com/responsive-web/client-web/";

/// GraphQL operations this backend knows how to invoke.
const NEEDED_OPERATIONS: [&str; 9] = [
    "CreateTweet",
    "TweetResultByRestId",
    "Viewer",
    "ViewerBadgeCounts",
    "SearchTimeline",
    "UserByScreenName",
    "UserTweets",
    "DeleteTweet",
    "UserByRestId",
];

#[derive(Debug, Clone)]
struct Operation {
    query_id: String,
    feature_switches: Vec<String>,
    field_toggles: Vec<String>,
}

/// Backend that calls the x.com internal web API using browser cookies.
///
/// Construct with [`WebBackend::new`]. The first request bootstraps the
/// GraphQL operation registry and feature-switch values from x.com's HTML
/// and `main.js`, so no endpoint ids are hardcoded.
pub struct WebBackend {
    client: Client,
    cookie_header: String,
    csrf_token: String,
    calls: Vec<CallRecord>,
    operations: Option<HashMap<String, Operation>>,
    features: Option<HashMap<String, Value>>,
    home_html: Option<String>,
    transaction: Option<twr_tx::ClientTransactionV1>,
}

impl WebBackend {
    /// Creates a backend using cookies extracted from a local browser.
    ///
    /// Tries Chromium-based browsers via `rookie` and requires `auth_token`
    /// and `ct0` cookies for `x.com`.
    pub fn new() -> Result<Self, BackendError> {
        let cookies = extract_browser_cookies()?;
        let csrf_token = cookies
            .iter()
            .find(|cookie| cookie.name == "ct0")
            .map(|cookie| cookie.value.clone())
            .ok_or_else(|| {
                BackendError::Http(
                    "browser session is missing the ct0 CSRF cookie; log in to x.com first"
                        .to_string(),
                )
            })?;
        let cookie_header = cookies
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(45))
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|error| BackendError::Http(error.to_string()))?;

        Ok(Self {
            client,
            cookie_header,
            csrf_token,
            calls: Vec::new(),
            operations: None,
            features: None,
            home_html: None,
            transaction: None,
        })
    }

    /// Headers for top-level document fetches. The API headers (bearer
    /// token, CSRF) must NOT be sent on page loads: x.com's edge returns
    /// 401 when they accompany an HTML request.
    fn page_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, HeaderValue::from_str(&self.cookie_header).unwrap());
        headers
    }

    /// Full headers for `i/api` GraphQL calls and `upload.x.com` media posts.
    /// `origin`/`referer`/`sec-fetch-*` are required on uploads.
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {WEB_BEARER}")).unwrap(),
        );
        headers.insert(COOKIE, HeaderValue::from_str(&self.cookie_header).unwrap());
        headers.insert(
            "x-csrf-token",
            HeaderValue::from_str(&self.csrf_token).unwrap(),
        );
        headers.insert(
            "x-twitter-auth-type",
            HeaderValue::from_static("OAuth2Session"),
        );
        headers.insert("x-twitter-active-user", HeaderValue::from_static("yes"));
        headers.insert("x-twitter-client-language", HeaderValue::from_static("en"));
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        headers.insert(ORIGIN, HeaderValue::from_static("https://x.com"));
        headers.insert(REFERER, HeaderValue::from_static("https://x.com/"));
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-site"));
        headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
        headers.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
        headers
    }

    /// Fetches the logged-in home page to collect feature-switch values and
    /// the `main.js` URL, then parses the operation registry out of `main.js`.
    fn bootstrap(&mut self) -> Result<(), BackendError> {
        if self.operations.is_some() && self.features.is_some() {
            return Ok(());
        }

        let response = self
            .client
            .get(HOME_URL)
            .headers(self.page_headers())
            .send()
            .map_err(|error| BackendError::Http(error.to_string()))?;
        let status = response.status();
        let html = response
            .text()
            .map_err(|error| BackendError::Http(error.to_string()))?;
        if !status.is_success() {
            return Err(BackendError::Http(format!(
                "{status}: could not load x.com home page; browser session may be expired"
            )));
        }

        self.features = Some(parse_feature_values(&html));
        self.home_html = Some(html.clone());

        let main_js_url = Regex::new(&format!(r#"{MAIN_JS_HOST}main\.[a-f0-9]+\.js"#))
            .ok()
            .and_then(|re| re.find(&html).map(|m| m.as_str().to_string()))
            .ok_or_else(|| {
                BackendError::Http("could not locate client-web main.js URL".to_string())
            })?;

        let main_js = self
            .client
            .get(&main_js_url)
            .send()
            .map_err(|error| BackendError::Http(error.to_string()))?
            .text()
            .map_err(|error| BackendError::Http(error.to_string()))?;

        self.operations = Some(parse_operations(&main_js, &NEEDED_OPERATIONS));
        Ok(())
    }

    fn transaction_id(&mut self, method: &str, path: &str) -> Result<String, BackendError> {
        if self.transaction.is_none() {
            let html = self
                .home_html
                .as_deref()
                .ok_or_else(|| BackendError::Http("missing bootstrapped homepage".into()))?;
            let key = twr_tx::extract_verification_key(html)
                .map_err(|e| BackendError::Http(e.to_string()))?;
            let frames = twr_tx::extract_frame_paths(html);
            let url = transaction_chunk_url(html)?;
            let script = self
                .client
                .get(url)
                .send()
                .map_err(|e| BackendError::Http(e.to_string()))?
                .error_for_status()
                .map_err(|e| BackendError::Http(e.to_string()))?
                .text()
                .map_err(|e| BackendError::Http(e.to_string()))?;
            let indices = twr_tx::extract_key_byte_indices(&script);
            if indices.len() < 2 || key.len() < 6 || indices.iter().any(|i| *i >= key.len()) {
                return Err(BackendError::Http(
                    "X transaction script has invalid key indices".into(),
                ));
            }
            let animation = twr_tx::compute_animation_key(&key, &frames, indices[0], &indices[1..])
                .map_err(|e| BackendError::Http(e.to_string()))?;
            self.transaction = Some(twr_tx::ClientTransactionV1::new(key, animation));
        }
        Ok(self.transaction.as_ref().unwrap().generate(method, path))
    }

    /// Runs a GraphQL operation and returns the `data` payload.
    fn graphql(
        &mut self,
        operation_name: &str,
        variables: Map<String, Value>,
    ) -> Result<Value, BackendError> {
        self.bootstrap()?;

        let operation = self
            .operations
            .as_ref()
            .and_then(|ops| ops.get(operation_name))
            .cloned()
            .ok_or_else(|| {
                BackendError::Http(format!(
                    "operation {operation_name} not found in web client registry"
                ))
            })?;

        let mut variables = variables;
        for toggle in &operation.field_toggles {
            variables.entry(toggle.clone()).or_insert(Value::Bool(true));
        }

        let mut features = Map::new();
        for switch in &operation.feature_switches {
            let value = self
                .features
                .as_ref()
                .and_then(|all| all.get(switch))
                .cloned()
                .unwrap_or(Value::Bool(false));
            features.insert(switch.clone(), value);
        }

        let body = serde_json::json!({
            "variables": Value::Object(variables),
            "features": Value::Object(features),
            "queryId": operation.query_id,
        });

        let url = format!("{GRAPHQL_BASE}/{}/{operation_name}", operation.query_id);
        self.calls.push(CallRecord {
            method: "POST_GRAPHQL".to_string(),
            path: format!("/i/api/graphql/{}/{operation_name}", operation.query_id),
            params: Vec::new(),
        });

        let mut request_headers = self.headers();
        if twr_tx::GATED_OPS.contains(&operation_name) {
            let path = format!("/i/api/graphql/{}/{operation_name}", operation.query_id);
            let id = self.transaction_id("GET", &path)?;
            request_headers.insert(
                "x-client-transaction-id",
                HeaderValue::from_str(&id).map_err(|e| BackendError::Http(e.to_string()))?,
            );
        }
        let request = if matches!(operation_name, "CreateTweet" | "DeleteTweet") {
            self.client.post(&url).headers(request_headers).json(&body)
        } else {
            self.client.get(&url).headers(request_headers).query(&[
                ("variables", body["variables"].to_string()),
                ("features", body["features"].to_string()),
                ("fieldToggles", serde_json::json!({"withArticleRichContentState":true,"withArticlePlainText":true}).to_string()),
            ])
        };
        let response = request
            .send()
            .map_err(|error| BackendError::Http(error.to_string()))?;

        let status = response.status();
        let text = response
            .text()
            .map_err(|error| BackendError::Http(error.to_string()))?;
        if !status.is_success() {
            return Err(BackendError::Http(format!(
                "{operation_name} ({}): {status}: {text}",
                operation.query_id
            )));
        }

        let value: Value = serde_json::from_str(&text)?;
        if let Some(errors) = value.get("errors").and_then(Value::as_array)
            && !errors.is_empty()
        {
            let message = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(BackendError::Http(format!(
                "GraphQL error: {}",
                if message.is_empty() { text } else { message }
            )));
        }

        Ok(value)
    }

    /// Uploads base64-encoded media and returns the `media_id_string`.
    fn upload_media_base64(&mut self, media_data: &str) -> Result<String, BackendError> {
        self.calls.push(CallRecord {
            method: "POST_UPLOAD".to_string(),
            path: "/i/media/upload.json".to_string(),
            params: Vec::new(),
        });

        let encoded = utf8_percent_encode(media_data, NON_ALPHANUMERIC).to_string();
        let response = self
            .client
            .post(MEDIA_UPLOAD_URL)
            .headers(self.headers())
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(format!("media_data={encoded}"))
            .send()
            .map_err(|error| BackendError::Http(error.to_string()))?;

        let status = response.status();
        let text = response
            .text()
            .map_err(|error| BackendError::Http(error.to_string()))?;
        if !status.is_success() {
            return Err(BackendError::Http(format!("{status}: {text}")));
        }

        let value: Value = serde_json::from_str(&text)?;
        value
            .get("media_id_string")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                BackendError::Http(format!("media upload returned no media_id_string: {text}"))
            })
    }

    /// Maps a `/2/tweets` JSON body onto the CreateTweet GraphQL mutation.
    fn create_tweet(&mut self, body: &Value) -> Result<Value, BackendError> {
        let text = body
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let media_entities: Vec<Value> = body
            .pointer("/media/media_ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(|id| {
                        serde_json::json!({
                            "media_id": id,
                            "tagged_users": [],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut variables = Map::new();
        variables.insert("tweet_text".to_string(), Value::String(text.clone()));
        variables.insert("dark_request".to_string(), Value::Bool(false));
        variables.insert(
            "media".to_string(),
            serde_json::json!({
                "media_entities": media_entities,
                "possibly_sensitive": false,
            }),
        );
        variables.insert(
            "semantic_annotation_ids".to_string(),
            Value::Array(Vec::new()),
        );
        if let Some(reply_to) = body
            .pointer("/reply/in_reply_to_tweet_id")
            .and_then(Value::as_str)
        {
            variables.insert(
                "reply".to_string(),
                serde_json::json!({
                    "in_reply_to_tweet_id": reply_to,
                    "exclude_reply_user_ids": [],
                }),
            );
        }

        let data = self.graphql("CreateTweet", variables)?;
        let rest_id = data
            .pointer("/data/create_tweet/tweet_results/result/rest_id")
            .or_else(|| data.pointer("/create_tweet/tweet_results/result/rest_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BackendError::Http(format!("CreateTweet returned no rest_id: {data}"))
            })?;

        Ok(serde_json::json!({
            "data": {
                "id": rest_id,
                "text": text,
            }
        }))
    }

    /// Reads the current browser account, without developer credentials.
    pub fn native_me(&mut self) -> Result<Value, BackendError> {
        let response = self.graphql(
            "Viewer",
            serde_json::json!({"withCommunitiesMemberships":true})
                .as_object()
                .unwrap()
                .clone(),
        )?;
        viewer_profile(&response)
    }

    /// Reads a public user profile by screen name.
    pub fn native_user(&mut self, username: &str) -> Result<Value, BackendError> {
        let response = self.graphql("UserByScreenName", serde_json::json!({"screen_name":username.trim_start_matches('@'),"withSafetyModeUserFields":true}).as_object().unwrap().clone())?;
        let user = response
            .pointer("/data/user/result")
            .ok_or_else(|| BackendError::Http("user not found".into()))?;
        if user.get("rest_id").is_none() {
            return Err(BackendError::Http("user unavailable".into()));
        }
        Ok(serde_json::json!({"data":normalize_user(user)}))
    }

    /// Reads a post including media and full note text.
    pub fn native_read(&mut self, id: &str) -> Result<Value, BackendError> {
        let response = self.graphql("TweetResultByRestId", serde_json::json!({"tweetId":id,"withCommunity":true,"includePromotedContent":false,"withVoice":true}).as_object().unwrap().clone())?;
        let result = response
            .pointer("/data/tweetResult/result")
            .and_then(normalize_tweet)
            .ok_or_else(|| BackendError::Http(format!("post {id} unavailable")))?;
        Ok(serde_json::json!({"data":result}))
    }

    /// Searches latest posts and returns a continuation cursor when available.
    pub fn native_search(
        &mut self,
        query: &str,
        count: u32,
        cursor: Option<&str>,
    ) -> Result<Value, BackendError> {
        let mut variables = serde_json::json!({"rawQuery":query,"count":count,"querySource":"typed_query","product":"Latest","withGrokTranslatedBio":false});
        if let Some(cursor) = cursor {
            variables["cursor"] = Value::String(cursor.into());
        }
        let response = self.graphql("SearchTimeline", variables.as_object().unwrap().clone())?;
        Ok(normalize_timeline(&response))
    }

    /// Reads a user's posts and returns a continuation cursor when available.
    pub fn native_timeline(
        &mut self,
        username: &str,
        count: u32,
        cursor: Option<&str>,
    ) -> Result<Value, BackendError> {
        let user = self.native_user(username)?;
        let mut variables = serde_json::json!({"userId":user["data"]["id"],"count":count,"includePromotedContent":false,"withQuickPromoteEligibilityTweetFields":true,"withVoice":true,"withV2Timeline":true});
        if let Some(cursor) = cursor {
            variables["cursor"] = Value::String(cursor.into());
        }
        let response = self.graphql("UserTweets", variables.as_object().unwrap().clone())?;
        Ok(normalize_timeline(&response))
    }

    /// Maps `GET /2/tweets/{id}` onto the TweetResultByRestId GraphQL query,
    /// returning the v1-ish shape the runner expects.
    fn tweet_result(&mut self, tweet_id: &str) -> Result<Value, BackendError> {
        let mut variables = Map::new();
        variables.insert("tweetId".to_string(), Value::String(tweet_id.to_string()));
        variables.insert("withCommunity".to_string(), Value::Bool(false));
        variables.insert("includePromotedContent".to_string(), Value::Bool(false));
        variables.insert("withVoice".to_string(), Value::Bool(false));

        let data = self.graphql("TweetResultByRestId", variables)?;
        let mut result = data
            .pointer("/data/tweetResult/result")
            .or_else(|| data.pointer("/tweetResult/result"))
            .cloned()
            .unwrap_or(Value::Null);
        // TweetWithVisibilityResults wraps the real tweet under `tweet`.
        if let Some(inner) = result.get("tweet").filter(|inner| !inner.is_null()) {
            result = inner.clone();
        }

        let screen_name = result
            .pointer("/core/user_results/result/core/screen_name")
            .or_else(|| result.pointer("/core/user_results/result/legacy/screen_name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let full_text = result
            .pointer("/legacy/full_text")
            .or_else(|| result.pointer("/note_tweet/note_tweet_results/result/text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(serde_json::json!({
            "data": {
                "id": tweet_id,
                "id_str": tweet_id,
                "text": full_text,
                "full_text": full_text,
                "user": { "screen_name": screen_name },
            }
        }))
    }
}

impl Backend for WebBackend {
    fn get_json(
        &mut self,
        path: &str,
        _params: Vec<(String, String)>,
    ) -> Result<Value, BackendError> {
        Err(BackendError::Http(format!(
            "web backend does not support GET {path}"
        )))
    }

    fn post_json(
        &mut self,
        path: &str,
        _params: Vec<(String, String)>,
    ) -> Result<Value, BackendError> {
        Err(BackendError::Http(format!(
            "web backend does not support POST params {path}"
        )))
    }

    fn post_json_body(&mut self, path: &str, body: Value) -> Result<Value, BackendError> {
        if path == "/2/tweets" {
            return self.create_tweet(&body);
        }
        if path == "/2/media/upload" {
            let media_data = body
                .get("media")
                .or_else(|| body.get("media_data"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let media_id = self.upload_media_base64(media_data)?;
            return Ok(serde_json::json!({ "data": { "id": media_id } }));
        }
        Err(BackendError::Http(format!(
            "web backend does not support POST body {path}"
        )))
    }

    fn post_json_body_oauth2(&mut self, path: &str, body: Value) -> Result<Value, BackendError> {
        self.post_json_body(path, body)
    }

    fn delete_json(
        &mut self,
        path: &str,
        _params: Vec<(String, String)>,
    ) -> Result<Value, BackendError> {
        if let Some(id) = path
            .strip_prefix("/2/tweets/")
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        {
            self.graphql(
                "DeleteTweet",
                serde_json::json!({"tweet_id":id,"dark_request":false})
                    .as_object()
                    .unwrap()
                    .clone(),
            )?;
            return Ok(serde_json::json!({"data":{"id":id,"deleted":true}}));
        }
        Err(BackendError::Http(format!(
            "web backend does not support DELETE {path}"
        )))
    }

    fn get_json_oauth2(
        &mut self,
        path: &str,
        _params: Vec<(String, String)>,
    ) -> Result<Value, BackendError> {
        if let Some(tweet_id) = path
            .strip_prefix("/2/tweets/")
            .filter(|id| id.chars().all(|c| c.is_ascii_digit()))
        {
            return self.tweet_result(tweet_id);
        }
        Err(BackendError::Http(format!(
            "web backend does not support GET {path}"
        )))
    }

    fn stream_json_lines(
        &mut self,
        path: &str,
        _params: Vec<(String, String)>,
        _auth: AuthScheme,
        _on_event: &mut dyn FnMut(Value) -> bool,
    ) -> Result<(), BackendError> {
        Err(BackendError::Http(format!(
            "web backend does not support STREAM {path}"
        )))
    }

    fn calls(&self) -> &[CallRecord] {
        &self.calls
    }
}

fn session_domain(domain: &str) -> bool {
    let d = domain.trim_start_matches('.');
    d == "x.com" || d.ends_with(".x.com") || d == "twitter.com" || d.ends_with(".twitter.com")
}

/// Extracts `x.com` cookies from local browser profiles.
fn extract_browser_cookies() -> Result<Vec<rookie::enums::Cookie>, BackendError> {
    let domains = vec!["x.com".to_string(), "twitter.com".to_string()];
    let mut last_error = String::new();
    for loader in [
        rookie::chrome,
        rookie::brave,
        rookie::edge,
        rookie::arc,
        rookie::vivaldi,
        rookie::chromium,
    ] {
        match loader(Some(domains.clone())) {
            Ok(cookies) => {
                let session: Vec<rookie::enums::Cookie> = cookies
                    .into_iter()
                    .filter(|cookie| session_domain(&cookie.domain))
                    .collect();
                if session.iter().any(|c| c.name == "auth_token")
                    && session.iter().any(|c| c.name == "ct0")
                {
                    return Ok(session);
                }
            }
            Err(error) => last_error = error.to_string(),
        }
    }

    Err(BackendError::Http(format!(
        "no logged-in x.com session found in local browsers (auth_token/ct0 missing). \
Log in to x.com in a supported browser first. {last_error}"
    )))
}

/// Parses `"name":{"value":X}` feature-switch entries out of the bootstrapped
/// HTML, keeping boolean values only.
fn parse_feature_values(html: &str) -> HashMap<String, Value> {
    let re = Regex::new(r#""([a-z0-9_]+)":\{"value":(true|false)\}"#).unwrap();
    re.captures_iter(html)
        .map(|cap| (cap[1].to_string(), Value::Bool(&cap[2] == "true")))
        .collect()
}

/// Parses the GraphQL operation registry out of `main.js`.
///
/// Each operation is declared as
/// `{queryId:"…",operationName:"…",operationType:"query|mutation",metadata:{featureSwitches:[…],fieldToggles:[…]}}`.
fn parse_operations(main_js: &str, wanted: &[&str]) -> HashMap<String, Operation> {
    let op_re = Regex::new(r#"queryId:"([A-Za-z0-9_-]+)",operationName:"([A-Za-z]+)""#).unwrap();
    let list_re = |block: &str, key: &str| -> Vec<String> {
        Regex::new(&format!(r#"{key}:\[([^\]]*)\]"#))
            .ok()
            .and_then(|re| re.captures(block).map(|c| c[1].to_string()))
            .map(|list| {
                list.split(',')
                    .filter_map(|item| item.trim().trim_matches('"').to_string().into())
                    .filter(|s: &String| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut operations = HashMap::new();
    for cap in op_re.captures_iter(main_js) {
        let name = cap[2].to_string();
        if !wanted.contains(&name.as_str()) {
            continue;
        }
        // The metadata object follows the operationName/operationType pair.
        let start = cap.get(0).unwrap().end();
        let block = &main_js[start..(start + 4096).min(main_js.len())];
        operations.insert(
            name,
            Operation {
                query_id: cap[1].to_string(),
                feature_switches: list_re(block, "featureSwitches"),
                field_toggles: list_re(block, "fieldToggles"),
            },
        );
    }
    operations
}

fn transaction_chunk_url(html: &str) -> Result<String, BackendError> {
    let names = Regex::new(r#"(\d+):["']ondemand\.s["']"#).unwrap();
    let id = names
        .captures(html)
        .ok_or_else(|| BackendError::Http("X homepage has no transaction chunk".into()))?;
    let hashes = Regex::new(&format!(r#"{}:["']([a-f0-9]+)["']"#, &id[1])).unwrap();
    let hash = hashes
        .captures(html)
        .ok_or_else(|| BackendError::Http("X homepage has no transaction chunk hash".into()))?;
    Ok(format!("{MAIN_JS_HOST}ondemand.s.{}a.js", &hash[1]))
}

fn normalize_user(user: &Value) -> Value {
    let mut output = user
        .get("legacy")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if !output.is_object() {
        output = serde_json::json!({});
    }
    output["id"] = user["rest_id"].clone();
    for key in ["screen_name", "name", "created_at"] {
        if let Some(value) = user.pointer(&format!("/core/{key}")) {
            output[key] = value.clone();
        }
    }
    for (key, path) in [
        ("description", "/profile_bio/description"),
        ("followers_count", "/relationship_counts/followers"),
        ("friends_count", "/relationship_counts/following"),
        ("statuses_count", "/tweet_counts/tweets"),
        ("media_count", "/tweet_counts/media_tweets"),
        ("profile_image_url_https", "/avatar/image_url"),
        ("location", "/location/location"),
        ("protected", "/privacy/protected"),
        ("verified", "/verification/verified"),
    ] {
        if let Some(value) = user.pointer(path) {
            output[key] = value.clone();
        }
    }
    output
}

fn normalize_tweet(result: &Value) -> Option<Value> {
    let result = result.get("tweet").unwrap_or(result);
    let id = result.get("rest_id")?.as_str()?;
    let text = result
        .pointer("/note_tweet/note_tweet_results/result/text")
        .or_else(|| result.pointer("/legacy/full_text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let user = result
        .pointer("/core/user_results/result")
        .map(normalize_user)
        .unwrap_or(Value::Null);
    Some(serde_json::json!({
        "id":id,"url":format!("https://x.com/i/status/{id}"),"text":text,"user":user,
        "created_at":result.pointer("/legacy/created_at"),
        "in_reply_to_status_id":result.pointer("/legacy/in_reply_to_status_id_str"),
        "media":result.pointer("/legacy/extended_entities/media").cloned().unwrap_or_else(||serde_json::json!([])),
        "metrics":{"replies":result.pointer("/legacy/reply_count"),"likes":result.pointer("/legacy/favorite_count"),"reposts":result.pointer("/legacy/retweet_count")}
    }))
}

fn normalize_timeline(response: &Value) -> Value {
    fn visit(value: &Value, posts: &mut Vec<Value>, cursor: &mut Option<String>) {
        match value {
            Value::Object(object) => {
                if object.get("cursorType").and_then(Value::as_str) == Some("Bottom") {
                    *cursor = object
                        .get("value")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                if let Some(result) = object.get("tweet_results").and_then(|v| v.get("result")) {
                    if let Some(post) = normalize_tweet(result)
                        && !posts.iter().any(|existing| existing["id"] == post["id"])
                    {
                        posts.push(post);
                    }
                    // Do not include quoted/retweeted nested posts as separate timeline entries.
                    return;
                }
                for value in object.values() {
                    visit(value, posts, cursor);
                }
            }
            Value::Array(array) => {
                for value in array {
                    visit(value, posts, cursor);
                }
            }
            _ => {}
        }
    }
    let mut posts = vec![];
    let mut cursor = None;
    visit(response, &mut posts, &mut cursor);
    serde_json::json!({"data":posts,"next_cursor":cursor})
}

#[cfg(test)]
mod native_tests {
    use super::*;
    #[test]
    fn account_profile_includes_current_schema_bio_and_counts() {
        let user = serde_json::json!({"rest_id":"1","core":{"name":"Tester","screen_name":"tester"},"profile_bio":{"description":"Rust"},"relationship_counts":{"followers":12,"following":3},"tweet_counts":{"tweets":4}});
        let value = normalize_user(&user);
        assert_eq!(value["description"], "Rust");
        assert_eq!(value["followers_count"], 12);
        assert_eq!(value["statuses_count"], 4);
    }
    #[test]
    fn discovers_transaction_chunk_from_webpack_tables() {
        let html =
            r#"<script>var names={59924:"ondemand.s"};var hashes={59924:"deadbeef"};</script>"#;
        assert_eq!(
            transaction_chunk_url(html).unwrap(),
            "https://abs.twimg.com/responsive-web/client-web/ondemand.s.deadbeefa.js"
        );
    }
    #[test]
    fn unavailable_post_is_not_a_successful_empty_read() {
        assert!(normalize_tweet(&serde_json::json!({"__typename":"TweetUnavailable"})).is_none());
    }
    #[test]
    fn normalizes_note_text_and_retains_media() {
        let tweet = serde_json::json!({"rest_id":"42", "legacy":{"full_text":"short", "extended_entities":{"media":[{"type":"photo"}]}}, "note_tweet":{"note_tweet_results":{"result":{"text":"complete"}}}, "core":{"user_results":{"result":{"legacy":{"screen_name":"tester"}}}}});
        let normalized = normalize_tweet(&tweet).unwrap();
        assert_eq!(normalized["text"], "complete");
        assert_eq!(normalized["media"][0]["type"], "photo");
    }
    #[test]
    fn timeline_collects_nested_posts_once_and_cursor() {
        let response = serde_json::json!({"entries":[{"content":{"itemContent":{"tweet_results":{"result":{"rest_id":"42", "legacy":{"full_text":"hello"}}}}}}, {"content":{"cursorType":"Bottom", "value":"next-page"}}]});
        let result = normalize_timeline(&response);
        assert_eq!(result["data"][0]["id"], "42");
        assert_eq!(result["next_cursor"], "next-page");
    }
}

#[cfg(test)]
mod login_domain_tests {
    #[test]
    fn ignores_lookalike_cookie_domains() {
        for d in ["x.com", ".x.com", "api.twitter.com"] {
            assert!(super::session_domain(d));
        }
        for d in ["notx.com", "x.com.evil.invalid", "twitter.com.evil.invalid"] {
            assert!(!super::session_domain(d));
        }
    }
}

fn viewer_profile(response: &Value) -> Result<Value, BackendError> {
    let result = response
        .pointer("/data/viewer/user_results/result")
        .or_else(|| response.pointer("/data/viewer/user_result/result"))
        .ok_or_else(|| BackendError::Http("Viewer returned no account".into()))?;
    if !result
        .get("rest_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
    {
        return Err(BackendError::Http(
            "Viewer returned no authenticated account ID".into(),
        ));
    }
    Ok(serde_json::json!({"data": normalize_user(result)}))
}

#[cfg(test)]
mod viewer_profile_tests {
    use super::*;
    #[test]
    fn rejects_null_empty_and_unavailable_accounts() {
        for result in [
            serde_json::json!(null),
            serde_json::json!({}),
            serde_json::json!({"__typename":"UserUnavailable"}),
        ] {
            assert!(
                viewer_profile(
                    &serde_json::json!({"data":{"viewer":{"user_results":{"result":result}}}})
                )
                .is_err()
            );
        }
    }
    #[test]
    fn accepts_a_real_account_id() {
        assert_eq!(viewer_profile(&serde_json::json!({"data":{"viewer":{"user_results":{"result":{"rest_id":"123","legacy":{"screen_name":"fixture"}}}}}})).unwrap()["data"]["id"],"123");
    }
}
