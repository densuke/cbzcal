use anyhow::{Result, bail};
use std::process::Command as ProcessCommand;

/// `url` が `http://` または `https://` で始まることを検証する。
/// それ以外のスキーム（`file:`, `javascript:` 等）はセキュリティリスクとなるため拒否する。
fn validate_browser_url(url: &str) -> Result<()> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        bail!("ブラウザで開けるのは http:// または https:// の URL のみです: {url}")
    }
}

pub fn open_in_browser(url: &str) -> Result<()> {
    validate_browser_url(url)?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = ProcessCommand::new("open");
        cmd.arg(url);
        cmd
    };

    #[cfg(target_os = "linux")]
    let mut command = {
        let mut cmd = ProcessCommand::new("xdg-open");
        cmd.arg(url);
        cmd
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = ProcessCommand::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    };

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("この OS では `--web` に未対応です");
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let status = command.status()?;
        if !status.success() {
            bail!("ブラウザ起動に失敗しました: {status}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_browser_url_accepts_http() {
        assert!(validate_browser_url("http://example.com").is_ok());
    }

    #[test]
    fn validate_browser_url_accepts_https() {
        assert!(validate_browser_url("https://example.com/path?q=1").is_ok());
    }

    #[test]
    fn validate_browser_url_rejects_file_scheme() {
        let err = validate_browser_url("file:///etc/passwd").unwrap_err();
        assert!(err.to_string().contains("http"));
    }

    #[test]
    fn validate_browser_url_rejects_javascript_scheme() {
        assert!(validate_browser_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn validate_browser_url_rejects_data_scheme() {
        assert!(validate_browser_url("data:text/html,<script>alert(1)</script>").is_err());
    }

    #[test]
    fn validate_browser_url_rejects_empty_string() {
        assert!(validate_browser_url("").is_err());
    }

    #[test]
    fn validate_browser_url_rejects_no_scheme() {
        assert!(validate_browser_url("example.com/path").is_err());
    }
}
