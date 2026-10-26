//! `loupe-models`: AI modellerini indirir ve doğrular (RFC-001G §3).
//!
//! ```text
//! loupe-models [--dir <dizin>] status    durum tablosu (indirmez)
//! loupe-models [--dir <dizin>] install   eksik ya da bozuk dosyaları indirir
//! loupe-models [--dir <dizin>] verify    her dosyayı SHA-256 ile doğrular; bozuksa 1 ile çıkar
//! loupe-models pin                       Hugging Face özetleriyle sabitlenmiş manifesti yazdırır
//! ```
//!
//! Varsayılan dizin: `$XDG_DATA_HOME/loupe/models` (`~/.local/share/loupe/models`).

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use loupe_ai::fetch::{Fetcher, Progress, pin};
use loupe_ai::models::{FileState, Manifest, default_dir, status};

fn mb(n: u64) -> String {
    format!("{:.1} MB", n as f64 / 1e6)
}

fn print_status(manifest: &Manifest, dir: &std::path::Path) -> Result<bool, loupe_ai::AiError> {
    let rows = status(manifest, dir)?;
    println!("{}", dir.display());
    for r in &rows {
        let state = match r.state {
            FileState::Verified => "doğrulandı",
            FileState::Missing => "yok",
            FileState::Corrupt => "BOZUK",
            FileState::Unpinned => "özet bilinmiyor",
            FileState::Incomplete => "AĞIRLIK EKSİK",
        };
        let size = r.size.map_or_else(|| "?".to_owned(), mb);
        println!("  {:<24} {:<11} {:>10}  {}", r.file, r.role, size, state);
    }
    Ok(rows.iter().all(|r| r.state == FileState::Verified))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("loupe-models: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode, loupe_ai::AiError> {
    let mut args = std::env::args().skip(1);
    let mut dir = default_dir();
    let mut cmd = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir = args.next().map(PathBuf::from),
            "-h" | "--help" => cmd = Some("help".to_owned()),
            other => cmd = Some(other.to_owned()),
        }
    }
    let manifest = Manifest::builtin()?;
    let dir = dir.ok_or_else(|| loupe_ai::AiError::InvalidInput("model dizini yok: --dir verin".into()))?;
    match cmd.as_deref().unwrap_or("status") {
        "status" => {
            print_status(&manifest, &dir)?;
            Ok(ExitCode::SUCCESS)
        }
        "verify" => Ok(if print_status(&manifest, &dir)? { ExitCode::SUCCESS } else { ExitCode::FAILURE }),
        "install" => {
            let fetcher = Fetcher::new()?;
            let progress = Arc::new(|p: Progress| {
                let pct = (100 * p.done).checked_div(p.total).unwrap_or(0);
                eprint!("\r  {:<32} {:<9} {:>3}%  {} / {}   ", p.source, p.phase, pct, mb(p.done), mb(p.total));
                if p.phase == "done" {
                    eprintln!();
                }
                let _ignored = std::io::stderr().flush();
            });
            let report = fetcher.install(&manifest, &dir, progress, Arc::new(AtomicBool::new(false))).await?;
            eprintln!(
                "kuruldu: {} · zaten vardı: {} · indirilen: {}",
                report.installed.len(),
                report.unchanged.len(),
                mb(report.downloaded_bytes)
            );
            for f in &report.failed {
                eprintln!("✗ {} ({}): {}", f.source, f.files.join(", "), f.reason);
            }
            let all = print_status(&manifest, &dir)?;
            Ok(if all && report.failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        "pin" => {
            let pinned = pin(&Fetcher::new()?, &manifest).await?;
            let json = serde_json::to_string_pretty(&pinned).map_err(|e| loupe_ai::AiError::Io(e.to_string()))?;
            println!("{json}");
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            println!("kullanım: loupe-models [--dir <dizin>] status|install|verify|pin");
            Ok(ExitCode::SUCCESS)
        }
    }
}
