use std::{borrow::Cow, fs, path::Path};

use http::{header, Request, Response, StatusCode};
use percent_encoding::percent_decode_str;
use tauri::{Manager, Runtime, UriSchemeContext};

use crate::{append_log, plugins, WidgetState};

const HOST_SCRIPT: &[u8] = include_bytes!("../../public/plugin-host.js");
const VUE_RUNTIME: &[u8] = include_bytes!("../../public/plugin-vue-runtime.js");
const VUE: &[u8] = include_bytes!("../../public/vendor/vue.global.prod.js");
const DAYJS: &[u8] = include_bytes!("../../public/vendor/dayjs.min.js");
const KUI_JS: &[u8] = include_bytes!("../../public/vendor/kui-vue.umd.js");
const KUI_CSS: &[u8] = include_bytes!("../../public/vendor/kui-vue.css");

fn response(status: StatusCode, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header("X-Content-Type-Options", "nosniff")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response<Vec<u8>> {
    response(status, "text/plain; charset=utf-8", message.into().into_bytes())
}

fn host_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    match path {
        "_host/plugin-host.js" => Some(("text/javascript; charset=utf-8", HOST_SCRIPT)),
        "_host/plugin-vue-runtime.js" => Some(("text/javascript; charset=utf-8", VUE_RUNTIME)),
        "_host/vendor/vue.global.prod.js" => Some(("text/javascript; charset=utf-8", VUE)),
        "_host/vendor/dayjs.min.js" => Some(("text/javascript; charset=utf-8", DAYJS)),
        "_host/vendor/kui-vue.umd.js" => Some(("text/javascript; charset=utf-8", KUI_JS)),
        "_host/vendor/kui-vue.css" => Some(("text/css; charset=utf-8", KUI_CSS)),
        _ => None,
    }
}

fn mime_type(path: &Path) -> &'static str {
    match path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn inject_host_runtime(html: String) -> Vec<u8> {
    const TAG: &str = r#"<script src="/_host/plugin-host.js"></script>"#;
    if html.contains(TAG) {
        return html.into_bytes();
    }

    let lower = html.to_ascii_lowercase();
    if let Some(head_start) = lower.find("<head") {
        if let Some(close) = lower[head_start..].find('>') {
            let insert_at = head_start + close + 1;
            let mut output = String::with_capacity(html.len() + TAG.len() + 1);
            output.push_str(&html[..insert_at]);
            output.push('\n');
            output.push_str(TAG);
            output.push_str(&html[insert_at..]);
            return output.into_bytes();
        }
    }

    format!("{TAG}\n{html}").into_bytes()
}

fn decoded_path(request: &Request<Vec<u8>>) -> Result<Cow<'_, str>, String> {
    let raw = request.uri().path().trim_start_matches('/');
    percent_decode_str(raw)
        .decode_utf8()
        .map_err(|error| format!("插件资源 URL 编码无效：{error}"))
}

pub(crate) fn handle<R: Runtime>(
    context: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let decoded = match decoded_path(&request) {
        Ok(value) => value,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    let path = decoded.trim_start_matches('/');

    if let Some((mime, bytes)) = host_asset(path) {
        return response(StatusCode::OK, mime, bytes.to_vec());
    }

    let mut parts = path.splitn(2, '/');
    let plugin_id = parts.next().unwrap_or_default();
    let relative = parts.next().unwrap_or_default();
    if plugin_id.is_empty() || relative.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "插件资源 URL 缺少 pluginId 或文件路径");
    }

    let state = context.app_handle().state::<WidgetState>();
    let file = match plugins::resolve_plugin_asset(state.inner(), plugin_id, relative) {
        Ok(path) => path,
        Err(error) => {
            append_log(
                &state.log_path,
                "ERROR",
                "plugin-assets",
                &format!("资源解析失败 plugin={plugin_id} path={relative} uri={} error={error}", request.uri()),
            );
            return error_response(StatusCode::NOT_FOUND, error);
        }
    };
    let mime = mime_type(&file);
    let bytes = match fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("无法读取插件资源 {}：{error}", file.display()),
            )
        }
    };

    if mime.starts_with("text/html") {
        append_log(
            &state.log_path,
            "DEBUG",
            "plugin-assets",
            &format!("加载 HTML plugin={plugin_id} path={relative} uri={}", request.uri()),
        );
        match String::from_utf8(bytes) {
            Ok(html) => response(StatusCode::OK, mime, inject_host_runtime(html)),
            Err(error) => error_response(StatusCode::BAD_REQUEST, format!("插件 HTML 不是 UTF-8：{error}")),
        }
    } else {
        response(StatusCode::OK, mime, bytes)
    }
}
