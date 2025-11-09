use std::io::{self, Write};

use anyhow::{Context, Result};
use arboard::Clipboard as ArboardClipboard;
use base64::{Engine as _, engine::general_purpose::STANDARD as Base64Standard};
use tracing::warn;

pub(super) trait ClipboardSink {
    fn set_text(&mut self, text: &str) -> Result<()>;
}

struct SystemClipboard {
    inner: ArboardClipboard,
}

impl SystemClipboard {
    fn new() -> Result<Self> {
        let inner = ArboardClipboard::new().context("クリップボードの初期化に失敗しました")?;
        Ok(Self { inner })
    }
}

impl ClipboardSink for SystemClipboard {
    fn set_text(&mut self, text: &str) -> Result<()> {
        self.inner
            .set_text(text.to_string())
            .context("クリップボードへの書き込みに失敗しました")
    }
}

struct Osc52Clipboard;

impl ClipboardSink for Osc52Clipboard {
    fn set_text(&mut self, text: &str) -> Result<()> {
        let sequence = osc52_sequence(text);
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(sequence.as_bytes())
            .context("OSC 52 シーケンスの送信に失敗しました")?;
        stdout
            .flush()
            .context("OSC 52 シーケンス送信後のフラッシュに失敗しました")?;
        Ok(())
    }
}

pub(super) fn osc52_sequence(text: &str) -> String {
    let encoded = Base64Standard.encode(text);
    format!("\x1b]52;c;{encoded}\x07")
}

pub(super) fn default_clipboard() -> Box<dyn ClipboardSink> {
    match SystemClipboard::new() {
        Ok(cb) => Box::new(cb),
        Err(err) => {
            warn!("システムクリップボードに接続できませんでした: {err}. OSC52へフォールバックします");
            Box::new(Osc52Clipboard)
        }
    }
}
