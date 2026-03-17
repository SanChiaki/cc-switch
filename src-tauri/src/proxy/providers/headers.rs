use crate::{
    config::{get_app_config_dir, write_text_file},
    provider::{Provider, RequestHeadersAuthMode},
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rquickjs::{Context, Function, Runtime};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

const HIS_TOKEN_SCRIPT_RELATIVE_PATH: &str = "scripts/provider-auth/his_token.js";
const HIS_TOKEN_MAX_CHAINED_REQUESTS: usize = 8;
const DEFAULT_HIS_TOKEN_SCRIPT: &str = r#"// inputHeaders:
//   The JSON object configured in the provider's "Request Headers" field.
//   In HIS token mode, these headers are used as script input only.
//
// return value:
//   The extractor must return a JSON object. That JSON becomes the actual
//   request headers written to the upstream model request.
//
// Quick start:
// 1. Put the first HIS endpoint in inputHeaders.getAccessTokenUrl.
// 2. Put the second HIS endpoint in inputHeaders.getDynamicTokenUrl.
// 3. Put app_key / app_secret / appid in the same JSON.
// 4. Return the final upstream headers from extractor().
const getAccessTokenUrl = inputHeaders.getAccessTokenUrl || "";
const getDynamicTokenUrl = inputHeaders.getDynamicTokenUrl || "";
const appKey = inputHeaders.app_key || "";
const appSecret = inputHeaders.app_secret || "";
const appId = inputHeaders.appid || "xxxxx";

if (!getAccessTokenUrl) {
  throw new Error(
    'Missing inputHeaders.getAccessTokenUrl. You can also edit scripts/provider-auth/his_token.js directly.',
  );
}

if (!getDynamicTokenUrl) {
  throw new Error(
    'Missing inputHeaders.getDynamicTokenUrl. You can also edit scripts/provider-auth/his_token.js directly.',
  );
}

if (!appKey || !appSecret) {
  throw new Error("Missing inputHeaders.app_key or inputHeaders.app_secret");
}

function readAccessToken(response) {
  return (
    response?.body?.data?.accessToken ||
    response?.body?.data?.AccessToken ||
    response?.body?.accessToken ||
    response?.body?.AccessToken ||
    ""
  );
}

function readDynamicToken(response) {
  return (
    response?.body?.data?.dynamicToken ||
    response?.body?.data?.DynamicToken ||
    response?.body?.dynamicToken ||
    response?.body?.DynamicToken ||
    response?.body?.token ||
    response?.body?.Authorization ||
    ""
  );
}

({
  nextRequest(previousResponse, requestHeaders, responses) {
    if (responses.length === 0) {
      return {
        url: getAccessTokenUrl,
        method: "POST",
        headers: {
          "content-type": "application/json",
        },
        body: {
          app_key: appKey,
          app_secret: appSecret,
        },
        timeoutSecs: 10,
      };
    }

    if (responses.length === 1) {
      if (!previousResponse.ok) {
        throw new Error(
          `getAccessToken failed with HTTP ${previousResponse.status}`,
        );
      }

      const accessToken = readAccessToken(previousResponse);
      if (!accessToken) {
        throw new Error("No accessToken found in getAccessToken response body");
      }

      return {
        url: getDynamicTokenUrl,
        method: "POST",
        headers: {
          "content-type": "application/json",
          AccessToken: accessToken,
        },
        body: {
          appid: appId,
        },
        timeoutSecs: 10,
      };
    }

    return null;
  },

  extractor(lastResponse, requestHeaders, responses) {
    if (!lastResponse.ok) {
      throw new Error(`getDynamicToken failed with HTTP ${lastResponse.status}`);
    }

    if (responses.length < 2) {
      throw new Error("Expected getAccessToken and getDynamicToken responses");
    }

    const dynamicToken = readDynamicToken(lastResponse);

    if (!dynamicToken) {
      throw new Error("No dynamicToken found in getDynamicToken response body");
    }

    return {
      Authorization: dynamicToken,
    };
  },
})
"#;

#[derive(Debug, Deserialize)]
struct ScriptRequestConfig {
    url: String,
    method: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(rename = "timeoutSecs", default)]
    timeout_secs: Option<u64>,
}

#[derive(Clone, Debug)]
struct ScriptHttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Value,
}

fn to_header_value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(v) => Some(v.clone()),
        Value::Number(v) => Some(v.to_string()),
        Value::Bool(v) => Some(v.to_string()),
        _ => None,
    }
}

fn normalize_header_name(source: &str, raw_name: &str) -> Result<String, String> {
    let name = raw_name.trim();
    if name.is_empty() {
        return Err(format!("{source} contains an empty header name"));
    }

    HeaderName::from_bytes(name.as_bytes())
        .map_err(|e| format!("{source} contains invalid header name '{name}': {e}"))?;

    Ok(name.to_string())
}

fn validate_header_value(source: &str, name: &str, raw_value: &str) -> Result<(), String> {
    HeaderValue::from_str(raw_value)
        .map_err(|e| format!("{source} contains invalid value for header '{name}': {e}"))?;
    Ok(())
}

fn insert_header(
    headers: &mut HeaderMap,
    source: &str,
    raw_name: &str,
    raw_value: &str,
) -> Result<(), String> {
    let name = normalize_header_name(source, raw_name)?;
    validate_header_value(source, &name, raw_value)?;

    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|e| format!("{source} contains invalid header name '{name}': {e}"))?;
    let header_value = HeaderValue::from_str(raw_value)
        .map_err(|e| format!("{source} contains invalid value for header '{name}': {e}"))?;

    headers.insert(header_name, header_value);
    Ok(())
}

fn insert_string_header(
    output: &mut HashMap<String, String>,
    source: &str,
    raw_name: &str,
    raw_value: &str,
) -> Result<(), String> {
    let name = normalize_header_name(source, raw_name)?;
    validate_header_value(source, &name, raw_value)?;
    output.insert(name, raw_value.to_string());
    Ok(())
}

fn merge_json_headers_into_strings(
    output: &mut HashMap<String, String>,
    source: &str,
    value: &Value,
) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Err(format!("{source} must be a JSON object"));
    };

    for (key, raw_value) in object {
        let Some(value_str) = to_header_value_string(raw_value) else {
            return Err(format!(
                "{source} header '{key}' must be a string, number, or boolean"
            ));
        };
        insert_string_header(output, source, key, &value_str)?;
    }

    Ok(())
}

fn merge_string_headers_into_strings(
    output: &mut HashMap<String, String>,
    source: &str,
    values: &HashMap<String, String>,
) -> Result<(), String> {
    for (key, value) in values {
        insert_string_header(output, source, key, value)?;
    }

    Ok(())
}

fn header_map_from_strings(
    source: &str,
    values: &HashMap<String, String>,
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for (key, value) in values {
        insert_header(&mut headers, source, key, value)?;
    }
    Ok(headers)
}

fn header_strings_from_json_value(
    source: &str,
    value: &Value,
) -> Result<HashMap<String, String>, String> {
    let mut headers = HashMap::new();
    merge_json_headers_into_strings(&mut headers, source, value)?;
    Ok(headers)
}

fn extract_custom_request_header_inputs(
    provider: &Provider,
) -> Result<HashMap<String, String>, String> {
    let mut headers = HashMap::new();

    if let Some(value) = provider.settings_config.get("headers") {
        merge_json_headers_into_strings(&mut headers, "settingsConfig.headers", value)?;
    }

    if let Some(value) = provider.settings_config.get("requestHeaders") {
        merge_json_headers_into_strings(&mut headers, "settingsConfig.requestHeaders", value)?;
    }

    if let Some(meta_headers) = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.request_headers.as_ref())
    {
        merge_string_headers_into_strings(&mut headers, "meta.requestHeaders", meta_headers)?;
    }

    Ok(headers)
}

fn his_token_script_path() -> PathBuf {
    get_app_config_dir().join(HIS_TOKEN_SCRIPT_RELATIVE_PATH)
}

fn ensure_his_token_script_exists() -> Result<PathBuf, String> {
    let path = his_token_script_path();
    if path.exists() {
        return Ok(path);
    }

    write_text_file(&path, DEFAULT_HIS_TOKEN_SCRIPT).map_err(|e| {
        format!(
            "Failed to create HIS token script at {}: {e}",
            path.display()
        )
    })?;

    Ok(path)
}

fn build_script_with_input(
    script_code: &str,
    input_headers: &HashMap<String, String>,
) -> Result<String, String> {
    let input_headers_json = serde_json::to_string(input_headers)
        .map_err(|e| format!("Failed to serialize request header inputs: {e}"))?;
    Ok(format!(
        "const inputHeaders = {input_headers_json};\n{script_code}"
    ))
}

fn serialize_script_response(response: &ScriptHttpResponse) -> Value {
    json!({
        "status": response.status,
        "ok": (200..=299).contains(&response.status),
        "headers": response.headers,
        "body": response.body,
    })
}

fn parse_script_request_config(request_json: &str) -> Result<ScriptRequestConfig, String> {
    let request: ScriptRequestConfig = serde_json::from_str(request_json)
        .map_err(|e| format!("Invalid HIS token request config format: {e}"))?;

    if request.url.trim().is_empty() {
        return Err("HIS token request.url cannot be empty".to_string());
    }

    Ok(request)
}

fn extract_next_script_request(
    script_code: &str,
    input_headers: &HashMap<String, String>,
    previous_response: Option<&ScriptHttpResponse>,
    responses: &[ScriptHttpResponse],
) -> Result<Option<ScriptRequestConfig>, String> {
    let script_with_input = build_script_with_input(script_code, input_headers)?;
    let previous_response_json = serde_json::to_string(
        &previous_response
            .map(serialize_script_response)
            .unwrap_or(Value::Null),
    )
    .map_err(|e| format!("Failed to serialize HIS token previous response payload: {e}"))?;
    let input_headers_json = serde_json::to_string(input_headers)
        .map_err(|e| format!("Failed to serialize request header inputs: {e}"))?;
    let responses_json = serde_json::to_string(
        &responses
            .iter()
            .map(serialize_script_response)
            .collect::<Vec<_>>(),
    )
    .map_err(|e| format!("Failed to serialize HIS token response payloads: {e}"))?;

    let request_json = {
        let runtime =
            Runtime::new().map_err(|e| format!("Failed to create HIS token JS runtime: {e}"))?;
        let context = Context::full(&runtime)
            .map_err(|e| format!("Failed to create HIS token JS context: {e}"))?;

        context.with(|ctx| {
            let config: rquickjs::Object = ctx
                .eval(script_with_input.clone())
                .map_err(|e| format!("Failed to evaluate HIS token script: {e}"))?;

            match config.get::<_, Function>("nextRequest") {
                Ok(next_request) => {
                    let previous_response_js: rquickjs::Value = ctx
                        .json_parse(previous_response_json.as_str())
                        .map_err(|e| {
                            format!("Failed to parse HIS token previous response payload: {e}")
                        })?;
                    let request_headers_js: rquickjs::Value = ctx
                        .json_parse(input_headers_json.as_str())
                        .map_err(|e| format!("Failed to parse request header inputs: {e}"))?;
                    let responses_js: rquickjs::Value = ctx
                        .json_parse(responses_json.as_str())
                        .map_err(|e| format!("Failed to parse HIS token response payloads: {e}"))?;

                    let request_js: rquickjs::Value = next_request
                        .call((previous_response_js, request_headers_js, responses_js))
                        .map_err(|e| format!("Failed to execute HIS token nextRequest: {e}"))?;

                    let Some(request_json) = ctx.json_stringify(request_js).map_err(|e| {
                        format!("Failed to serialize HIS token nextRequest output: {e}")
                    })?
                    else {
                        return Ok::<_, String>(None);
                    };

                    let request_json: String = request_json
                        .get()
                        .map_err(|e| format!("Failed to read HIS token request JSON: {e}"))?;

                    if request_json.trim() == "null" {
                        return Ok(None);
                    }

                    Ok(Some(request_json))
                }
                Err(_) => {
                    if previous_response.is_some() {
                        return Ok(None);
                    }

                    let request: rquickjs::Object = config
                        .get("request")
                        .map_err(|e| format!("Missing request config in HIS token script: {e}"))?;

                    let request_json: String = ctx
                        .json_stringify(request)
                        .map_err(|e| format!("Failed to serialize HIS token request config: {e}"))?
                        .ok_or_else(|| {
                            "HIS token request config serialization returned None".to_string()
                        })?
                        .get()
                        .map_err(|e| format!("Failed to read HIS token request JSON: {e}"))?;

                    Ok(Some(request_json))
                }
            }
        })?
    };

    request_json
        .map(|value| parse_script_request_config(&value))
        .transpose()
}

#[cfg_attr(not(test), allow(dead_code))]
fn extract_script_request(
    script_code: &str,
    input_headers: &HashMap<String, String>,
) -> Result<ScriptRequestConfig, String> {
    extract_next_script_request(script_code, input_headers, None, &[])?
        .ok_or_else(|| "HIS token script did not produce an initial request config".to_string())
}

fn extract_script_headers_from_responses(
    script_code: &str,
    input_headers: &HashMap<String, String>,
    responses: &[ScriptHttpResponse],
) -> Result<HashMap<String, String>, String> {
    let script_with_input = build_script_with_input(script_code, input_headers)?;
    let last_response = responses
        .last()
        .ok_or_else(|| "HIS token script did not receive any responses".to_string())?;
    let response_json = serde_json::to_string(&serialize_script_response(last_response))
        .map_err(|e| format!("Failed to serialize HIS token response payload: {e}"))?;
    let input_headers_json = serde_json::to_string(input_headers)
        .map_err(|e| format!("Failed to serialize request header inputs: {e}"))?;
    let responses_json = serde_json::to_string(
        &responses
            .iter()
            .map(serialize_script_response)
            .collect::<Vec<_>>(),
    )
    .map_err(|e| format!("Failed to serialize HIS token response payloads: {e}"))?;

    let result: Value = {
        let runtime =
            Runtime::new().map_err(|e| format!("Failed to create HIS token JS runtime: {e}"))?;
        let context = Context::full(&runtime)
            .map_err(|e| format!("Failed to create HIS token JS context: {e}"))?;

        context.with(|ctx| {
            let config: rquickjs::Object = ctx
                .eval(script_with_input.clone())
                .map_err(|e| format!("Failed to evaluate HIS token script: {e}"))?;
            let extractor: Function = config
                .get("extractor")
                .map_err(|e| format!("Missing extractor function in HIS token script: {e}"))?;

            let response_js: rquickjs::Value = ctx
                .json_parse(response_json.as_str())
                .map_err(|e| format!("Failed to parse HIS token response payload: {e}"))?;
            let request_headers_js: rquickjs::Value =
                ctx.json_parse(input_headers_json.as_str())
                    .map_err(|e| format!("Failed to parse request header inputs: {e}"))?;
            let responses_js: rquickjs::Value = ctx
                .json_parse(responses_json.as_str())
                .map_err(|e| format!("Failed to parse HIS token response payloads: {e}"))?;

            let result_js: rquickjs::Value = extractor
                .call((response_js, request_headers_js, responses_js))
                .map_err(|e| format!("Failed to execute HIS token extractor: {e}"))?;

            let result_json: String = ctx
                .json_stringify(result_js)
                .map_err(|e| format!("Failed to serialize HIS token extractor output: {e}"))?
                .ok_or_else(|| "HIS token extractor must return a JSON object".to_string())?
                .get()
                .map_err(|e| format!("Failed to read HIS token extractor JSON: {e}"))?;

            serde_json::from_str(&result_json)
                .map_err(|e| format!("Failed to parse HIS token extractor output JSON: {e}"))
        })?
    };

    header_strings_from_json_value("hisToken.extractor", &result)
}

#[cfg_attr(not(test), allow(dead_code))]
fn extract_script_headers(
    script_code: &str,
    input_headers: &HashMap<String, String>,
    response: &ScriptHttpResponse,
) -> Result<HashMap<String, String>, String> {
    extract_script_headers_from_responses(
        script_code,
        input_headers,
        &[ScriptHttpResponse {
            status: response.status,
            headers: response.headers.clone(),
            body: response.body.clone(),
        }],
    )
}

async fn send_script_request(
    provider: &Provider,
    config: &ScriptRequestConfig,
) -> Result<ScriptHttpResponse, String> {
    let method: reqwest::Method = config.method.parse().map_err(|_| {
        format!(
            "HIS token script contains unsupported HTTP method: {}",
            config.method
        )
    })?;

    let proxy_config = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.proxy_config.as_ref());
    let client = crate::proxy::http_client::get_for_provider(proxy_config);
    let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(10).clamp(2, 30));

    let mut request = client.request(method, &config.url).timeout(timeout);

    for (key, value) in &config.headers {
        let normalized_key = normalize_header_name("hisToken.request.headers", key)?;
        validate_header_value("hisToken.request.headers", &normalized_key, value)?;
        request = request.header(&normalized_key, value);
    }

    if let Some(body) = &config.body {
        request = match body {
            Value::String(value) => request.body(value.clone()),
            other => request.json(other),
        };
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("HIS token request failed: {e}"))?;

    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect::<HashMap<_, _>>();
    let body_text = response
        .text()
        .await
        .map_err(|e| format!("Failed to read HIS token response body: {e}"))?;
    let body = serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));

    Ok(ScriptHttpResponse {
        status: status.as_u16(),
        headers,
        body,
    })
}

async fn resolve_his_token_headers(
    provider: &Provider,
    input_headers: &HashMap<String, String>,
) -> Result<HeaderMap, String> {
    let script_path = ensure_his_token_script_exists()?;
    let script_code = fs::read_to_string(&script_path).map_err(|e| {
        format!(
            "Failed to read HIS token script at {}: {e}",
            script_path.display()
        )
    })?;

    let mut responses = Vec::new();

    loop {
        if responses.len() >= HIS_TOKEN_MAX_CHAINED_REQUESTS {
            return Err(format!(
                "HIS token script exceeded the maximum chained request limit ({HIS_TOKEN_MAX_CHAINED_REQUESTS})"
            ));
        }

        let request =
            extract_next_script_request(&script_code, input_headers, responses.last(), &responses)?;
        let Some(request) = request else {
            break;
        };

        let response = send_script_request(provider, &request).await?;
        responses.push(response);
    }

    if responses.is_empty() {
        return Err("HIS token script did not produce any HTTP request".to_string());
    }

    let generated_headers =
        extract_script_headers_from_responses(&script_code, input_headers, &responses)?;

    header_map_from_strings("hisToken.extractor", &generated_headers)
}

#[cfg_attr(not(test), allow(dead_code))]
fn extract_custom_request_headers(provider: &Provider) -> Result<HeaderMap, String> {
    let input_headers = extract_custom_request_header_inputs(provider)?;
    header_map_from_strings("custom request headers", &input_headers)
}

pub async fn resolve_custom_request_headers(provider: &Provider) -> Result<HeaderMap, String> {
    let input_headers = extract_custom_request_header_inputs(provider)?;

    match provider
        .meta
        .as_ref()
        .and_then(|meta| meta.request_headers_auth_mode.as_ref())
    {
        Some(RequestHeadersAuthMode::HisToken) => {
            resolve_his_token_headers(provider, &input_headers).await
        }
        None => header_map_from_strings("custom request headers", &input_headers),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderMeta;
    use serde_json::json;

    fn create_provider(config: serde_json::Value, meta: Option<ProviderMeta>) -> Provider {
        Provider {
            id: "test".to_string(),
            name: "Test Provider".to_string(),
            settings_config: config,
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    #[test]
    fn extracts_headers_from_provider_meta() {
        let mut request_headers = HashMap::new();
        request_headers.insert("x-test-token".to_string(), "token".to_string());

        let provider = create_provider(
            json!({}),
            Some(ProviderMeta {
                request_headers: Some(request_headers),
                ..ProviderMeta::default()
            }),
        );

        let headers = extract_custom_request_headers(&provider).unwrap();
        assert_eq!(
            headers.get("x-test-token").unwrap().to_str().unwrap(),
            "token"
        );
    }

    #[test]
    fn meta_headers_override_legacy_settings_headers() {
        let mut request_headers = HashMap::new();
        request_headers.insert("x-test-token".to_string(), "from-meta".to_string());

        let provider = create_provider(
            json!({
                "headers": {
                    "x-test-token": "from-settings"
                },
                "requestHeaders": {
                    "x-other": "value"
                }
            }),
            Some(ProviderMeta {
                request_headers: Some(request_headers),
                ..ProviderMeta::default()
            }),
        );

        let headers = extract_custom_request_headers(&provider).unwrap();
        assert_eq!(
            headers.get("x-test-token").unwrap().to_str().unwrap(),
            "from-meta"
        );
        assert_eq!(headers.get("x-other").unwrap().to_str().unwrap(), "value");
    }

    #[test]
    fn rejects_invalid_header_name() {
        let provider = create_provider(
            json!({
                "requestHeaders": {
                    "bad header": "value"
                }
            }),
            None,
        );

        let error = extract_custom_request_headers(&provider).unwrap_err();
        assert!(error.contains("invalid header name"));
    }

    #[test]
    fn his_token_script_can_transform_input_headers() {
        let input_headers = HashMap::from([
            ("id".to_string(), "user-1".to_string()),
            ("token".to_string(), "seed-token".to_string()),
            (
                "x-his-token-url".to_string(),
                "https://example.com/his/token".to_string(),
            ),
        ]);
        let script = r#"({
  request: {
    url: inputHeaders["x-his-token-url"],
    method: "POST",
    body: {
      id: inputHeaders.id,
      token: inputHeaders.token,
    },
  },
  extractor(response, requestHeaders) {
    return {
      "x-his-token": `${requestHeaders.id}:${response.body.token}`,
    };
  },
})"#;

        let request = extract_script_request(script, &input_headers).unwrap();
        assert_eq!(request.url, "https://example.com/his/token");
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.body,
            Some(json!({
                "id": "user-1",
                "token": "seed-token",
            }))
        );

        let generated = extract_script_headers(
            script,
            &input_headers,
            &ScriptHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: json!({
                    "token": "dynamic-token",
                }),
            },
        )
        .unwrap();

        assert_eq!(
            generated.get("x-his-token").map(String::as_str),
            Some("user-1:dynamic-token")
        );
    }

    #[test]
    fn his_token_script_output_must_be_json_object() {
        let input_headers = HashMap::new();
        let script = r#"({
  request: {
    url: "https://example.com/his/token",
    method: "GET",
  },
  extractor() {
    return ["not-an-object"];
  },
})"#;

        let error = extract_script_headers(
            script,
            &input_headers,
            &ScriptHttpResponse {
                status: 200,
                headers: HashMap::new(),
                body: json!({}),
            },
        )
        .unwrap_err();

        assert!(error.contains("must be a JSON object"));
    }

    #[test]
    fn his_token_script_can_chain_requests() {
        let input_headers = HashMap::from([
            (
                "getAccessTokenUrl".to_string(),
                "https://example.com/his/access-token".to_string(),
            ),
            (
                "getDynamicTokenUrl".to_string(),
                "https://example.com/his/dynamic-token".to_string(),
            ),
            ("app_key".to_string(), "key-1".to_string()),
            ("app_secret".to_string(), "secret-1".to_string()),
            ("appid".to_string(), "app-1".to_string()),
        ]);
        let script = r#"({
  nextRequest(previousResponse, requestHeaders, responses) {
    if (responses.length === 0) {
      return {
        url: requestHeaders.getAccessTokenUrl,
        method: "POST",
        body: {
          app_key: requestHeaders.app_key,
          app_secret: requestHeaders.app_secret,
        },
      };
    }

    if (responses.length === 1) {
      return {
        url: requestHeaders.getDynamicTokenUrl,
        method: "POST",
        headers: {
          AccessToken: previousResponse.body.accessToken,
        },
        body: {
          appid: requestHeaders.appid,
        },
      };
    }

    return null;
  },

  extractor(lastResponse, requestHeaders, responses) {
    return {
      Authorization: `${requestHeaders.appid}:${responses[0].body.accessToken}:${lastResponse.body.dynamicToken}`,
    };
  },
})"#;

        let request_1 = extract_next_script_request(script, &input_headers, None, &[])
            .unwrap()
            .expect("first request");
        assert_eq!(request_1.url, "https://example.com/his/access-token");
        assert_eq!(
            request_1.body,
            Some(json!({
                "app_key": "key-1",
                "app_secret": "secret-1",
            }))
        );

        let response_1 = ScriptHttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: json!({
                "accessToken": "access-1",
            }),
        };

        let request_2 = extract_next_script_request(
            script,
            &input_headers,
            Some(&response_1),
            std::slice::from_ref(&response_1),
        )
        .unwrap()
        .expect("second request");
        assert_eq!(request_2.url, "https://example.com/his/dynamic-token");
        assert_eq!(
            request_2.headers.get("AccessToken").map(String::as_str),
            Some("access-1")
        );
        assert_eq!(
            request_2.body,
            Some(json!({
                "appid": "app-1",
            }))
        );

        let response_1 = ScriptHttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: json!({
                "accessToken": "access-1",
            }),
        };
        let response_2 = ScriptHttpResponse {
            status: 200,
            headers: HashMap::new(),
            body: json!({
                "dynamicToken": "dynamic-1",
            }),
        };
        let responses = vec![response_1, response_2];

        let request_3 =
            extract_next_script_request(script, &input_headers, responses.last(), &responses)
                .unwrap();
        assert!(request_3.is_none());

        let generated =
            extract_script_headers_from_responses(script, &input_headers, &responses).unwrap();
        assert_eq!(
            generated.get("Authorization").map(String::as_str),
            Some("app-1:access-1:dynamic-1")
        );
    }
}
