use std::{
    ffi::OsStr,
    fs,
    path::Path,
};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProxyFile {
    pub proxy: String,
}

pub fn cli_proxy<I, S>(args: I) -> Result<Option<String>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut proxy = None;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        if argument.as_ref() != OsStr::new("--proxy") {
            continue;
        }
        if proxy.is_some() {
            return Err("命令行参数 --proxy 不能重复使用".to_string());
        }
        let value = args
            .next()
            .ok_or_else(|| "命令行参数 --proxy 缺少代理地址".to_string())?;
        if value.as_ref() == OsStr::new("--proxy") {
            return Err("命令行参数 --proxy 缺少代理地址".to_string());
        }
        proxy = Some(value.as_ref().to_string_lossy().into_owned());
    }
    Ok(proxy)
}

pub fn resolve_proxy(cli: Option<String>, file: Option<ProxyFile>) -> Option<String> {
    match cli {
        Some(proxy) => normalize_proxy(proxy),
        None => file.and_then(|file| normalize_proxy(file.proxy)),
    }
}

pub fn load_proxy_file(path: &Path) -> Result<Option<ProxyFile>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("无法读取代理配置文件 {}：{error}", path.display()))?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| format!("代理配置文件 {} 格式无效：{error}", path.display()))
}

fn normalize_proxy(proxy: String) -> Option<String> {
    let proxy = proxy.trim();
    (!proxy.is_empty()).then(|| proxy.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_proxy_reads_separate_value() {
        let proxy = cli_proxy(["widget", "--proxy", "http://127.0.0.1:7890"])
            .expect("separate --proxy value should be accepted");

        assert_eq!(proxy.as_deref(), Some("http://127.0.0.1:7890"));
    }

    #[test]
    fn cli_proxy_rejects_missing_value() {
        let error = cli_proxy(["widget", "--proxy"])
            .expect_err("--proxy without a value must be rejected");

        assert!(error.contains("缺少代理地址"));
    }

    #[test]
    fn cli_proxy_rejects_repeated_flag() {
        let error = cli_proxy([
            "widget",
            "--proxy",
            "http://127.0.0.1:7890",
            "--proxy",
            "http://127.0.0.1:7891",
        ])
        .expect_err("repeated --proxy must be rejected");

        assert!(error.contains("不能重复使用"));
    }

    #[test]
    fn resolve_proxy_prefers_cli_value() {
        let proxy = resolve_proxy(
            Some("http://127.0.0.1:7890".to_string()),
            Some(ProxyFile {
                proxy: "http://127.0.0.1:7891".to_string(),
            }),
        );

        assert_eq!(proxy.as_deref(), Some("http://127.0.0.1:7890"));
    }

    #[test]
    fn resolve_proxy_falls_back_to_file_value() {
        let proxy = resolve_proxy(
            None,
            Some(ProxyFile {
                proxy: "http://127.0.0.1:7890".to_string(),
            }),
        );

        assert_eq!(proxy.as_deref(), Some("http://127.0.0.1:7890"));
    }

    #[test]
    fn resolve_proxy_treats_blank_values_as_no_proxy() {
        let cli_proxy = resolve_proxy(
            Some("  ".to_string()),
            Some(ProxyFile {
                proxy: "http://127.0.0.1:7890".to_string(),
            }),
        );
        let file_proxy = resolve_proxy(
            None,
            Some(ProxyFile {
                proxy: "\t".to_string(),
            }),
        );

        assert_eq!(cli_proxy, None);
        assert_eq!(file_proxy, None);
    }

    #[test]
    fn load_proxy_file_allows_missing_file() {
        let path = std::env::temp_dir().join("tbwidget-missing-proxy-config.json");

        assert_eq!(load_proxy_file(&path).expect("missing file is allowed"), None);
    }
}
