use clap::{Parser, Subcommand};
use serde::Deserialize;
use colored::*;
use prettytable::{Table, Row, Cell};
use std::process::Command;
use std::path::Path;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{stdin, stdout, Write};
use std::time::Duration;
use std::env;

#[derive(Parser, Debug)]
#[command(name = "raur")]
#[command(version, about = "AUR + Pacman helper written in Rust", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Search for packages
    Search {
        query: String,
        #[arg(long, help = "Search only official Pacman repos")]
        pacman_only: bool,
        #[arg(long, help = "Search only AUR")]
        aur_only: bool,
    },
    /// Install a package
    Install {
        packages: Vec<String>,
        #[arg(short = 'c', long = "cascade")]
        cascade: bool,
    },
    /// Remove a package
    Remove {
        packages: Vec<String>,
        #[arg(long)]
        purge: bool,
    },
    /// Sync database
    Update {
        #[arg(short = 'y', long)]
        full: bool,
    },
    /// Upgrade system + AUR
    Upgrade {
        #[arg(short = 'y', long)]
        full: bool,
    },
}

#[derive(Debug, Deserialize)]
struct AurResponse {
    resultcount: i32,
    results: Vec<AurPackage>,
}

#[derive(Debug, Deserialize)]
struct AurPackage {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "Description")]
    description: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Search { query, pacman_only, aur_only } => {
            search_packages(query, *pacman_only, *aur_only).await?
        }
        Commands::Install { packages, cascade } => {
            for pkg in packages {
                install_package(pkg, *cascade).await?;
            }
        }
        Commands::Remove { packages, purge } => {
            for pkg in packages {
                remove_package(pkg, *purge)?;
            }
        }
        Commands::Update { full } => update_database(*full)?,
        Commands::Upgrade { full } => upgrade_system(*full).await?,
    }

    Ok(())
}

// ======================
// Search: Pacman + AUR
// ======================
async fn search_packages(query: &str, pacman_only: bool, aur_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Searching for '{}'...", query.blue());

    if !aur_only {
        // 1️⃣ Offizielle Repos
        let pacman_output = Command::new("pacman")
            .args(&["-Ss", query])
            .output()?;

        if !pacman_output.stdout.is_empty() {
            println!("📦 Found in official repos:");
            let stdout = String::from_utf8_lossy(&pacman_output.stdout);
            for line in stdout.lines().take(10) {
                println!("  {}", line.green());
            }
        } else {
            println!("⚠️ Not found in official repos");
        }
    }

    if !pacman_only {
        // 2️⃣ AUR
        let url = format!("https://aur.archlinux.org/rpc/?v=5&type=search&arg={}", query);
        let resp = reqwest::get(&url).await?.json::<AurResponse>().await?;

        if resp.resultcount > 0 {
            println!("🌐 Found {} packages in AUR:", resp.resultcount);
            let mut table = Table::new();
            table.add_row(Row::new(vec![
                Cell::new("Name"),
                Cell::new("Version"),
                Cell::new("Description"),
            ]));

            for pkg in resp.results.iter().take(10) {
                table.add_row(Row::new(vec![
                    Cell::new(&pkg.name.green().to_string()),
                    Cell::new(&pkg.version.yellow().to_string()),
                    Cell::new(&pkg.description.clone().unwrap_or_else(|| "No description".into())),
                ]));
            }

            table.printstd();
        } else {
            println!("❌ No packages found in AUR");
        }
    }

    Ok(())
}

// ======================
// Install: Pacman first, then AUR
// ======================
async fn install_package(pkgname: &str, cascade: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Prüfen, ob Paket im offiziellen Repo existiert
    let pacman_check = Command::new("pacman")
        .args(&["-Ss", pkgname])
        .output()?;

    if !pacman_check.stdout.is_empty() {
        println!("📦 Installing '{}' from official repos", pkgname.green());
        let status = Command::new("sudo")
            .arg("pacman")
            .args(&["-S", pkgname, "--noconfirm"])
            .status()?;

        if status.success() {
            println!("✅ Installed '{}' from official repos", pkgname.green());
        } else {
            println!("❌ Failed to install '{}' from official repos", pkgname.red());
        }
        return Ok(());
    }

    // Wenn nicht vorhanden, AUR-Build
    println!("🌐 '{}' not found in official repos, building from AUR", pkgname.yellow());

    let home_dir = env::var("HOME").unwrap_or("/tmp".to_string());
    let cache_dir = format!("{}/.cache/raur", home_dir);
    if !Path::new(&cache_dir).exists() {
        std::fs::create_dir_all(&cache_dir)?;
    }
    let temp_dir = format!("{}/{}", cache_dir, pkgname);
    if Path::new(&temp_dir).exists() {
        std::fs::remove_dir_all(&temp_dir)?;
    }

    let status = Command::new("git")
        .args(&["clone", &format!("https://aur.archlinux.org/{}.git", pkgname), &temp_dir])
        .status()?;
    if !status.success() {
        eprintln!("❌ Git clone failed");
        return Ok(());
    }

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner} {msg}")
            .unwrap()
            .tick_strings(&["⠁","⠂","⠄","⡀","⢀","⠠","⠐","⠈"])
    );
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message("Building package...");

    let makepkg_args = if cascade { vec!["-sci", "--noconfirm"] } else { vec!["-si", "--noconfirm"] };

    let status = Command::new("makepkg")
        .current_dir(&temp_dir)
        .args(&makepkg_args)
        .status()?;
    pb.finish_and_clear();

    if status.success() {
        println!("✅ Installed '{}' from AUR", pkgname.green());
    } else {
        println!("❌ Failed to install '{}' from AUR", pkgname.red());
    }

    Ok(())
}

// ======================
// Remove / Purge
// ======================
fn remove_package(pkgname: &str, purge: bool) -> Result<(), Box<dyn std::error::Error>> {
    print!("⚠️  Are you sure you want to remove '{}'? [y/N]: ", pkgname);
    stdout().flush()?;
    let mut input = String::new();
    stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() != "y" {
        println!("Aborted");
        return Ok(());
    }

    let args = if purge { vec!["-Rns", pkgname, "--noconfirm"] } else { vec!["-Rs", pkgname, "--noconfirm"] };

    let status = Command::new("sudo")
        .arg("pacman")
        .args(&args)
        .status()?;

    if status.success() {
        println!("✅ Removed '{}'", pkgname.green());
    } else {
        println!("❌ Failed to remove '{}'", pkgname.red());
    }

    Ok(())
}

// ======================
// Update / Sync
// ======================
fn update_database(full: bool) -> Result<(), Box<dyn std::error::Error>> {
    let pacman_args = if full { vec!["-Syy"] } else { vec!["-Sy"] };

    let status = Command::new("sudo")
        .arg("pacman")
        .args(&pacman_args)
        .status()?;

    if status.success() {
        println!("✅ Database synced successfully");
    } else {
        println!("❌ Database sync failed");
    }
    Ok(())
}

// ======================
// Upgrade
// ======================
async fn upgrade_system(full: bool) -> Result<(), Box<dyn std::error::Error>> {
    update_database(full)?;

    let status = Command::new("sudo")
        .arg("pacman")
        .args(&["-Syu", "--noconfirm"])
        .status()?;

    if status.success() {
        println!("✅ System upgraded successfully");
    } else {
        println!("❌ Upgrade failed");
    }

    Ok(())
}
