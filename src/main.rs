use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use plist::{Dictionary, Value};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod cg;

// Schema we were written against. Bump if Apple changes keys.
const KNOWN_TOP_KEYS: &[&str] = &["AllSpacesAndDisplays", "Displays", "Spaces", "SystemDefault"];

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "heic", "heif", "tif", "tiff", "gif", "bmp"];

#[derive(Parser)]
#[command(
    name = "macos-wp",
    about = "Manage macOS wallpapers per display. Changes survive newly-created Spaces.",
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

    /// Set a wallpaper. Defaults to all displays, sticky across new Spaces.
    Set {
        /// Path to the image file.
        path: PathBuf,
        /// Display alias (`builtin`, `ext-1`, `offline-1`, ...) or UUID. Default: all.
        #[arg(long)]
        display: Option<String>,
        /// Limit to one Space UUID (transient; does NOT stick to new Spaces).
        #[arg(long)]
        space: Option<String>,
    },

    /// Pick a random image from a directory and set it (same flags as `set`).
    Random {
        /// Directory containing images.
        dir: PathBuf,
        #[arg(long)]
        display: Option<String>,
        #[arg(long)]
        space: Option<String>,
    },

    /// Re-sync per-Space overrides to match each display's top-level default.
    Reset {
        /// Limit to one display (alias or UUID). Default: all.
        #[arg(long)]
        display: Option<String>,
    },

    /// Restore the plist from the last `.bak` written by macos-wp.
    Restore,
}

fn default_plist_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home)
        .join("Library/Application Support/com.apple.wallpaper/Store/Index.plist"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let plist_path = cli.plist.clone().unwrap_or(default_plist_path()?);

    // Restore handled before the plist read/schema-check because the primary plist
    // may be corrupted (the reason to restore in the first place).
    if let Cmd::Restore = cli.cmd {
        return cmd_restore(&plist_path);
    }

    let root = read_plist(&plist_path)?;
    let root_dict = root
        .as_dictionary()
        .ok_or_else(|| anyhow!("root of {} is not a dictionary", plist_path.display()))?;
    check_schema(root_dict, cli.force_schema)?;

    let aliases = DisplayAliases::build(root_dict);

    match cli.cmd {
        Cmd::List => cmd_list(root_dict, &aliases),
        Cmd::Set { path, display, space } => {
            let targets = aliases.resolve_many(display.as_deref())?;
            let blob = build_configuration_blob(&resolve_image(&path)?)?;
            run_write(&plist_path, root, |r| apply_blob(r, &blob, &targets, space.as_deref()))
        }
        Cmd::Random { dir, display, space } => {
            let targets = aliases.resolve_many(display.as_deref())?;
            let picked = pick_random_image(&dir)?;
            println!("picked: {}", picked.display());
            let blob = build_configuration_blob(&picked)?;
            run_write(&plist_path, root, |r| apply_blob(r, &blob, &targets, space.as_deref()))
        }
        Cmd::Reset { display } => {
            let targets = aliases.resolve_many(display.as_deref())?;
            run_write(&plist_path, root, |r| apply_reset(r, &targets))
        }
        Cmd::Restore => unreachable!("handled earlier"),
    }
}

fn cmd_restore(plist_path: &Path) -> Result<()> {
    let bak = plist_path.with_extension("plist.bak");
    if !bak.is_file() {
        bail!("no backup found at {}", bak.display());
    }
    fs::copy(&bak, plist_path)
        .with_context(|| format!("copying {} -> {}", bak.display(), plist_path.display()))?;
    let _ = Command::new("killall").arg("WallpaperAgent").status();
    println!("restored {} from {}", plist_path.display(), bak.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// plist IO
// ---------------------------------------------------------------------------

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

fn run_write(
    plist_path: &Path,
    mut root: Value,
    f: impl FnOnce(&mut Value) -> Result<usize>,
) -> Result<()> {
    let touched = f(&mut root)?;
    let bak = plist_path.with_extension("plist.bak");
    fs::copy(plist_path, &bak)
        .with_context(|| format!("backing up to {}", bak.display()))?;
    write_plist_atomic(plist_path, &root)?;
    let _ = Command::new("killall").arg("WallpaperAgent").status();
    println!("updated {} configuration slot(s)", touched);
    println!("backup: {}", bak.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Display aliases
// ---------------------------------------------------------------------------

struct DisplayAliases {
    // Ordered list: (alias, uuid). `uuid` is the plist UUID.
    entries: Vec<(String, String)>,
}

impl DisplayAliases {
    fn build(root: &Dictionary) -> Self {
        let plist_uuids: Vec<String> = root
            .get("Displays")
            .and_then(|v| v.as_dictionary())
            .map(|d| d.keys().cloned().collect())
            .unwrap_or_default();

        let online = cg::online_displays(); // Vec<(id, is_builtin, uuid)>

        let mut entries: Vec<(String, String)> = Vec::new();
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();

        // 1. builtin (online)
        if let Some((_, _, uuid)) = online.iter().find(|(_, b, _)| *b) {
            if plist_uuids.iter().any(|u| u == uuid) {
                entries.push(("builtin".into(), uuid.clone()));
                used.insert(uuid.clone());
            }
        }
        // 2. ext-N: online non-builtin, in discovery order
        let mut ext_n = 1;
        for (_, is_builtin, uuid) in &online {
            if *is_builtin || used.contains(uuid) {
                continue;
            }
            if plist_uuids.iter().any(|u| u == uuid) {
                entries.push((format!("ext-{}", ext_n), uuid.clone()));
                used.insert(uuid.clone());
                ext_n += 1;
            }
        }
        // 3. offline-N: plist UUIDs not matched above, sorted for stability
        let mut offline: Vec<String> = plist_uuids
            .iter()
            .filter(|u| !used.contains(*u))
            .cloned()
            .collect();
        offline.sort();
        for (i, uuid) in offline.into_iter().enumerate() {
            entries.push((format!("offline-{}", i + 1), uuid));
        }
        Self { entries }
    }

    fn resolve(&self, input: &str) -> Result<String> {
        // 1. Exact alias match.
        if let Some((_, uuid)) = self.entries.iter().find(|(a, _)| a == input) {
            return Ok(uuid.clone());
        }
        // 2. Exact UUID match (case-insensitive).
        if let Some((_, uuid)) = self
            .entries
            .iter()
            .find(|(_, u)| u.eq_ignore_ascii_case(input))
        {
            return Ok(uuid.clone());
        }
        bail!(
            "unknown display `{}`. Try one of: {}",
            input,
            self.entries
                .iter()
                .map(|(a, _)| a.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    /// Resolve `--display`: `None` => all; `Some("all")` => all; else one.
    fn resolve_many(&self, input: Option<&str>) -> Result<Vec<String>> {
        match input {
            None | Some("all") => Ok(self.entries.iter().map(|(_, u)| u.clone()).collect()),
            Some(s) => Ok(vec![self.resolve(s)?]),
        }
    }

    fn alias_for(&self, uuid: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(_, u)| u == uuid)
            .map(|(a, _)| a.as_str())
    }
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn cmd_list(root: &Dictionary, aliases: &DisplayAliases) -> Result<()> {
    let displays = root
        .get("Displays")
        .and_then(|v| v.as_dictionary())
        .ok_or_else(|| anyhow!("no Displays key"))?;

    println!("Displays:");
    for (uuid, entry) in displays {
        let alias = aliases.alias_for(uuid).unwrap_or("?");
        let file = entry
            .as_dictionary()
            .and_then(|d| d.get("Desktop"))
            .and_then(|v| v.as_dictionary())
            .and_then(extract_configuration_url)
            .unwrap_or_else(|| "<none>".to_string());
        println!("  {:<11}  {}", alias, uuid);
        println!("               -> {}", file);
    }

    if let Some(spaces) = root.get("Spaces").and_then(|v| v.as_dictionary()) {
        println!("\nSpaces: {}", spaces.len());
    }
    Ok(())
}

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

// ---------------------------------------------------------------------------
// Configuration blob encoding
// ---------------------------------------------------------------------------

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

fn resolve_image(path: &Path) -> Result<PathBuf> {
    let abs = path
        .canonicalize()
        .with_context(|| format!("resolving {}", path.display()))?;
    if !abs.is_file() {
        bail!("{} is not a file", abs.display());
    }
    Ok(abs)
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

// ---------------------------------------------------------------------------
// write primitives
// ---------------------------------------------------------------------------

fn apply_blob(
    root: &mut Value,
    blob: &[u8],
    displays: &[String],
    space: Option<&str>,
) -> Result<usize> {
    let root_dict = root
        .as_dictionary_mut()
        .ok_or_else(|| anyhow!("root not a dict"))?;
    let mut touched = 0;
    let sticky = space.is_none();

    if sticky {
        let top = root_dict
            .get_mut("Displays")
            .and_then(|v| v.as_dictionary_mut())
            .ok_or_else(|| anyhow!("no Displays key"))?;
        for uuid in displays {
            if let Some(e) = top.get_mut(uuid).and_then(|v| v.as_dictionary_mut()) {
                touched += set_entry_configuration(e, blob);
            }
        }
    }

    if let Some(spaces) = root_dict
        .get_mut("Spaces")
        .and_then(|v| v.as_dictionary_mut())
    {
        let mut matched_space = false;
        for (sp_uuid, sp_val) in spaces.iter_mut() {
            if let Some(filter) = space {
                if sp_uuid != filter {
                    continue;
                }
                matched_space = true;
            }
            let Some(sp_dict) = sp_val.as_dictionary_mut() else {
                continue;
            };
            let Some(inner) = sp_dict
                .get_mut("Displays")
                .and_then(|v| v.as_dictionary_mut())
            else {
                continue;
            };
            for uuid in displays {
                if let Some(e) = inner.get_mut(uuid).and_then(|v| v.as_dictionary_mut()) {
                    touched += set_entry_configuration(e, blob);
                }
            }
        }
        if let Some(filter) = space {
            if !matched_space {
                bail!("space `{}` not found", filter);
            }
        }
    }

    Ok(touched)
}

fn apply_reset(root: &mut Value, displays: &[String]) -> Result<usize> {
    let root_dict = root
        .as_dictionary_mut()
        .ok_or_else(|| anyhow!("root not a dict"))?;

    // Snapshot the top-level blob for each target display first, so the mutable borrow
    // for Spaces doesn't overlap.
    let mut per_display_blob: Vec<(String, Vec<u8>)> = Vec::new();
    {
        let top = root_dict
            .get("Displays")
            .and_then(|v| v.as_dictionary())
            .ok_or_else(|| anyhow!("no Displays key"))?;
        for uuid in displays {
            let Some(entry) = top.get(uuid).and_then(|v| v.as_dictionary()) else {
                continue;
            };
            let Some(desktop) = entry.get("Desktop").and_then(|v| v.as_dictionary()) else {
                continue;
            };
            let Some(data) = desktop
                .get("Content")
                .and_then(|v| v.as_dictionary())
                .and_then(|d| d.get("Choices"))
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_dictionary())
                .and_then(|d| d.get("Configuration"))
                .and_then(|v| v.as_data())
            else {
                continue;
            };
            per_display_blob.push((uuid.clone(), data.to_vec()));
        }
    }

    if per_display_blob.is_empty() {
        bail!("no top-level Configuration found for targeted display(s); nothing to sync from");
    }

    let mut touched = 0;
    let spaces = match root_dict
        .get_mut("Spaces")
        .and_then(|v| v.as_dictionary_mut())
    {
        Some(s) => s,
        None => return Ok(0),
    };
    for (_sp_uuid, sp_val) in spaces.iter_mut() {
        let Some(sp_dict) = sp_val.as_dictionary_mut() else {
            continue;
        };
        let Some(inner) = sp_dict
            .get_mut("Displays")
            .and_then(|v| v.as_dictionary_mut())
        else {
            continue;
        };
        for (uuid, blob) in &per_display_blob {
            if let Some(e) = inner.get_mut(uuid).and_then(|v| v.as_dictionary_mut()) {
                touched += set_entry_configuration(e, blob);
            }
        }
    }
    Ok(touched)
}

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

// ---------------------------------------------------------------------------
// random image picker
// ---------------------------------------------------------------------------

fn pick_random_image(dir: &Path) -> Result<PathBuf> {
    let md = fs::metadata(dir).with_context(|| format!("stat {}", dir.display()))?;
    if !md.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if IMAGE_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                candidates.push(p);
            }
        }
    }
    if candidates.is_empty() {
        bail!(
            "no images in {} (extensions: {})",
            dir.display(),
            IMAGE_EXTS.join(", ")
        );
    }
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let idx = (seed as usize) % candidates.len();
    Ok(candidates.swap_remove(idx))
}
