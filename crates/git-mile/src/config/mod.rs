//! Configuration module for git-mile.

use anyhow::{Context, Result, anyhow};
use std::io::{self, Write};
use std::path::Path;

pub mod keybindings;

pub use keybindings::{Action, KeyBindingsConfig, ViewType};

/// Initialize keybindings configuration file with defaults.
pub fn init_keybindings(output: Option<&Path>, force: bool) -> Result<()> {
    // Determine output path
    let output_path = match output {
        Some(path) => path.to_path_buf(),
        None => keybindings::default_config_path()
            .ok_or_else(|| anyhow!("設定ディレクトリを特定できませんでした"))?,
    };

    // Write configuration file
    write_keybindings_config(&output_path, force)?;

    Ok(())
}

fn write_keybindings_config(path: &Path, force: bool) -> Result<()> {
    // Check if file exists and force is false
    if path.exists() && !force {
        if !confirm_overwrite(path)? {
            println!("中止しました。");
            return Ok(());
        }
    }

    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("ディレクトリの作成に失敗しました: {}", parent.display()))?;
    }

    // Generate TOML content
    let content = keybindings::generate_default_keybindings_toml()?;

    // Write to file
    std::fs::write(path, content)
        .with_context(|| format!("設定ファイルの書き込みに失敗しました: {}", path.display()))?;

    println!("✓ キーバインド設定ファイルを作成しました: {}", path.display());
    println!();
    println!("キーバインドをカスタマイズするには、このファイルを編集してください。");
    println!("変更を適用するには、git-mile tui を再起動してください。");

    Ok(())
}

fn confirm_overwrite(path: &Path) -> Result<bool> {
    print!("ファイルがすでに存在します: {}\n上書きしますか? [y/N]: ", path.display());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}
