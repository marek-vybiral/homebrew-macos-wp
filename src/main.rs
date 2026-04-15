use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use plist::{Dictionary, Value};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

// Schema we were written against. Bump if Apple changes keys.
const KNOWN_TOP_KEYS: &[&str] = &["AllSpacesAndDisplays", "Displays", "Spaces", "SystemDefault"];

#[derive(Parser)]
#[command(
    name = "macos-wp",
    about = "Manage macOS wallpapers per display, survives new Spaces.",
    version
)]
struct Cli {
    /// Bypass plist schema validation (dangerous; Apple may have changed the format).
    #[arg(long, global = true)]
    force_schema: bool,

    /// Override plist path (defaults to ~/Library/Application Support/com.apple.wallpaper/Store/Index.plist).
    #[arg(long, global = true)]
    plist: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List displays and their current wallpaper.
    List,
    /// Set wallpaper for a display on all Spaces (existing + future).
    Set {
        /// Path to the image file.
        path: PathBuf,
        /// Display UUID (see `macos-wp list`).
        #[arg(long)]
        display: String,
    },
}

fn default_plist_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home)
        .join("Library/Application Support/com.apple.wallpaper/Store/Index.plist"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let plist_path = match &cli.plist {
        Some(p) => p.clone(),
        None => default_plist_path()?,
    };

    let root = read_plist(&plist_path)?;
    let root_dict = root
        .as_dictionary()
        .ok_or_else(|| anyhow!("root of {} is not a dictionary", plist_path.display()))?;

    check_schema(root_dict, cli.force_schema)?;

    match cli.cmd {
        Cmd::List => cmd_list(root_dict),
        Cmd::Set { path, display } => cmd_set(&plist_path, root.clone(), &path, &display),
    }
}

fn read_plist(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    plist::from_bytes(&bytes).with_context(|| format!("parsing {}", path.display()))
}

fn check_schema(root: &Dictionary, force: bool) -> Result<()> {
    let missing: Vec<&&str> = KNOWN_TOP_KEYS
        .iter()
        .filter(|k| !root.contains_key(**k))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    if force {
        eprintln!(
            "warning: plist is missing expected keys {:?}; continuing due to --force-schema",
            missing
        );
        return Ok(());
    }
    bail!(
        "plist schema unexpected (missing keys: {:?}). This tool was written against macOS Tahoe's com.apple.wallpaper format. \
         Rerun with --force-schema to try anyway (may corrupt your wallpaper settings; a .bak will be written).",
        missing
    );
}

fn cmd_list(root: &Dictionary) -> Result<()> {
    let displays = root
        .get("Displays")
        .and_then(|v| v.as_dictionary())
        .ok_or_else(|| anyhow!("no Displays key"))?;

    println!("Displays:");
    for (uuid, entry) in displays {
        let file = entry
            .as_dictionary()
            .and_then(|d| d.get("Desktop"))
            .and_then(|v| v.as_dictionary())
            .and_then(extract_configuration_url)
            .unwrap_or_else(|| "<none>".to_string());
        println!("  {uuid}  -> {file}");
    }

    if let Some(spaces) = root.get("Spaces").and_then(|v| v.as_dictionary()) {
        println!("\nSpaces: {}", spaces.len());
    }
    Ok(())
}

/// Given a `Desktop` dict (with `Content.Choices[0].Configuration` data blob),
/// decode the URL inside.
fn extract_configuration_url(desktop: &Dictionary) -> Option<String> {
    let data = desktop
        .get("Content")?
        .as_dictionary()?
        .get("Choices")?
        .as_array()?
        .first()?
        .as_dictionary()?
        .get("Configuration")?
        .as_data()?;
    let inner: Value = plist::from_bytes(data).ok()?;
    let d = inner.as_dictionary()?;
    let url = d.get("url")?.as_dictionary()?.get("relative")?.as_string()?;
    Some(url.to_string())
}

// URL encoding set matching macOS's file:// paths (spaces become %20, etc.).
const PATH_ENCODE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'%');

fn path_to_file_url(path: &Path) -> Result<String> {
    let s = path
        .to_str()
        .ok_or_else(|| anyhow!("non-UTF-8 path: {}", path.display()))?;
    Ok(format!("file://{}", utf8_percent_encode(s, PATH_ENCODE)))
}

fn build_configuration_blob(image_path: &Path) -> Result<Vec<u8>> {
    let url = path_to_file_url(image_path)?;

    let mut url_dict = Dictionary::new();
    url_dict.insert("relative".into(), Value::String(url));

    let mut config = Dictionary::new();
    config.insert("type".into(), Value::String("imageFile".into()));
    config.insert("url".into(), Value::Dictionary(url_dict));

    let mut buf: Vec<u8> = Vec::new();
    plist::to_writer_binary(Cursor::new(&mut buf), &Value::Dictionary(config))
        .context("serializing Configuration blob")?;
    Ok(buf)
}

fn cmd_set(plist_path: &Path, mut root: Value, image_path: &Path, display_uuid: &str) -> Result<()> {
    let image_path = image_path
        .canonicalize()
        .with_context(|| format!("resolving {}", image_path.display()))?;
    if !image_path.is_file() {
        bail!("{} is not a file", image_path.display());
    }

    let blob = build_configuration_blob(&image_path)?;

    let root_dict = root
        .as_dictionary_mut()
        .ok_or_else(|| anyhow!("root not a dict"))?;

    let mut touched = 0usize;

    // 1. Top-level Displays.<uuid> — the per-display default used for new Spaces.
    if let Some(displays) = root_dict.get_mut("Displays").and_then(|v| v.as_dictionary_mut()) {
        if let Some(entry) = displays.get_mut(display_uuid).and_then(|v| v.as_dictionary_mut()) {
            touched += set_entry_configuration(entry, &blob);
        } else {
            bail!(
                "display {} not found in top-level Displays (run `macos-wp list`)",
                display_uuid
            );
        }
    } else {
        bail!("no Displays key in plist");
    }

    // 2. Existing Spaces.*.Displays.<uuid> overrides.
    if let Some(spaces) = root_dict.get_mut("Spaces").and_then(|v| v.as_dictionary_mut()) {
        for (_space_uuid, space_entry) in spaces.iter_mut() {
            let Some(space_dict) = space_entry.as_dictionary_mut() else { continue };
            let Some(inner_displays) = space_dict
                .get_mut("Displays")
                .and_then(|v| v.as_dictionary_mut())
            else {
                continue;
            };
            if let Some(entry) = inner_displays
                .get_mut(display_uuid)
                .and_then(|v| v.as_dictionary_mut())
            {
                touched += set_entry_configuration(entry, &blob);
            }
        }
    }

    // Backup & write.
    let bak = plist_path.with_extension("plist.bak");
    fs::copy(plist_path, &bak)
        .with_context(|| format!("backing up to {}", bak.display()))?;

    write_plist_atomic(plist_path, &root)?;

    // Restart WallpaperAgent to pick up changes.
    let _ = Command::new("killall").arg("WallpaperAgent").status();

    println!(
        "updated {} configuration slot(s) for display {}",
        touched, display_uuid
    );
    println!("backup: {}", bak.display());
    Ok(())
}

/// Set `Configuration` under both `Desktop` and `Idle` sub-entries of `entry`.
/// Returns the number of slots written.
fn set_entry_configuration(entry: &mut Dictionary, blob: &[u8]) -> usize {
    let mut n = 0;
    for sub in ["Desktop", "Idle"] {
        if let Some(sub_dict) = entry.get_mut(sub).and_then(|v| v.as_dictionary_mut()) {
            if replace_configuration(sub_dict, blob) {
                n += 1;
            }
        }
    }
    n
}

fn replace_configuration(sub_dict: &mut Dictionary, blob: &[u8]) -> bool {
    let Some(content) = sub_dict
        .get_mut("Content")
        .and_then(|v| v.as_dictionary_mut())
    else {
        return false;
    };
    let Some(choices) = content.get_mut("Choices").and_then(|v| v.as_array_mut()) else {
        return false;
    };
    let Some(first) = choices.first_mut() else {
        return false;
    };
    let Some(choice) = first.as_dictionary_mut() else {
        return false;
    };
    choice.insert("Configuration".into(), Value::Data(blob.to_vec()));
    true
}

fn write_plist_atomic(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension("plist.tmp");
    {
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        plist::to_writer_binary(&mut f, value)
            .with_context(|| format!("writing {}", tmp.display()))?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}
