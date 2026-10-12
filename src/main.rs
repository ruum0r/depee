use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, ValueEnum};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Flex, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, Padding, Paragraph, Row, Table, TableState},
    DefaultTerminal, Frame,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{process::Command, sync::mpsc, time::timeout};

#[derive(Parser)]
#[command(
    name = "depee",
    version,
    about = "Inspect and update Node.js dependencies"
)]
struct Cli {
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long, value_enum)]
    pm: Option<Pm>,
    #[arg(long)]
    clear_cache: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
enum Pm {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}
impl Pm {
    fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Prod,
    Dev,
    Optional,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Patch,
    Minor,
    Major,
    Pre,
    Unknown,
}

#[derive(Clone, Debug)]
struct Release {
    version: String,
    published: Option<DateTime<Utc>>,
    change: Change,
    prerelease: bool,
    dependencies: Vec<String>,
}
#[derive(Clone, Debug)]
struct Package {
    name: String,
    current: String,
    wanted: Option<String>,
    latest: Option<String>,
    kind: Kind,
    change: Change,
    description: Option<String>,
    license: Option<String>,
    repo: Option<String>,
    homepage: Option<String>,
    current_published: Option<DateTime<Utc>>,
    published: Option<DateTime<Utc>>,
    downloads: Option<u64>,
    releases: Vec<Release>,
    release_index: usize,
}
#[derive(Deserialize)]
struct Manifest {
    name: Option<String>,
    #[serde(rename = "packageManager")]
    package_manager: Option<String>,
    dependencies: Option<HashMap<String, String>>,
    #[serde(rename = "devDependencies")]
    dev: Option<HashMap<String, String>>,
    #[serde(rename = "optionalDependencies")]
    optional: Option<HashMap<String, String>>,
}
#[derive(Clone, Debug)]
struct Metadata {
    description: Option<String>,
    license: Option<String>,
    repo: Option<String>,
    homepage: Option<String>,
    published: Option<DateTime<Utc>>,
    downloads: Option<u64>,
    releases: Vec<Release>,
}
#[derive(Serialize, Deserialize)]
struct UpdateCheckCache {
    checked_at: DateTime<Utc>,
    latest_version: Option<String>,
    #[serde(default)]
    skipped_version: Option<String>,
}
struct UpdatePrompt {
    version: String,
    selected: usize,
}
const UPDATE_CHOICES: [&str; 3] = ["Update now", "Skip this version", "Remind me later"];
const INSTALL_SCRIPT_URL: &str = "https://raw.githubusercontent.com/ruum0r/depee/main/install.sh";
enum Msg {
    Scan(Result<Vec<Package>, String>),
    Metadata(usize, Result<Metadata, String>),
    PmVersion(String),
    Update(Result<String, String>),
}
type Outdated = (String, String, Option<String>, Option<String>);
#[derive(Clone, Copy)]
enum Sort {
    Name,
    Change,
    Published,
}
struct App {
    cwd: PathBuf,
    project: String,
    pm: Pm,
    pm_version: String,
    total: usize,
    packages: Vec<Package>,
    selected: usize,
    query: Option<String>,
    sort: Sort,
    descending: bool,
    loading: bool,
    updating: bool,
    confirm: bool,
    version_picker: bool,
    selected_target: Option<String>,
    target_latest: bool,
    upgrade: bool,
    status: String,
    update_prompt: Option<UpdatePrompt>,
    install_update: Option<String>,
    error: Option<String>,
    metadata_loading: HashMap<usize, bool>,
    metadata_requested: HashSet<usize>,
    metadata_limit: Arc<tokio::sync::Semaphore>,
    tx: mpsc::UnboundedSender<Msg>,
}

impl App {
    fn visible(&self) -> Vec<usize> {
        let mut ids: Vec<usize> = self
            .packages
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                self.query
                    .as_ref()
                    .map(|q| p.name.to_lowercase().contains(&q.to_lowercase()))
                    .unwrap_or(true)
            })
            .map(|(i, _)| i)
            .collect();
        let pending_metadata = self.metadata_loading.values().any(|pending| *pending);
        let sort = if pending_metadata && matches!(self.sort, Sort::Published) {
            Sort::Name
        } else {
            self.sort
        };
        ids.sort_by(|a, b| {
            let (a, b) = (&self.packages[*a], &self.packages[*b]);
            let primary = match sort {
                Sort::Name => a.name.cmp(&b.name),
                Sort::Change => rank(a.change).cmp(&rank(b.change)),
                Sort::Published => match (a.published, b.published) {
                    (Some(a), Some(b)) => a.cmp(&b),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                },
            };
            let primary = if self.descending {
                match (sort, a.published.is_some(), b.published.is_some()) {
                    // Keep packages without a publication date last in either direction.
                    (Sort::Published, true, false) | (Sort::Published, false, true) => primary,
                    _ => primary.reverse(),
                }
            } else {
                primary
            };
            primary.then_with(|| a.name.cmp(&b.name))
        });
        ids
    }
    fn preserve_selection(&mut self, name: Option<String>) {
        let Some(name) = name else { return };
        if let Some(index) = self
            .visible()
            .into_iter()
            .find(|index| self.packages[*index].name == name)
        {
            self.selected = index;
        }
    }
    fn request_metadata(&mut self) {
        self.request_metadata_for(self.selected);
    }
    fn request_metadata_for(&mut self, index: usize) {
        if self.metadata_requested.contains(&index) {
            return;
        }
        if let Some(p) = self.packages.get(index) {
            self.metadata_requested.insert(index);
            self.metadata_loading.insert(index, true);
            let name = p.name.clone();
            let tx = self.tx.clone();
            let limit = self.metadata_limit.clone();
            tokio::spawn(async move {
                let Ok(_permit) = limit.acquire_owned().await else {
                    return;
                };
                let _ = tx.send(Msg::Metadata(
                    index,
                    metadata(&name).await.map_err(|e| format!("{e:#}")),
                ));
            });
        }
    }
    fn move_selection(&mut self, d: isize) {
        let visible = self.visible();
        if visible.is_empty() {
            return;
        }
        let pos = visible
            .iter()
            .position(|x| *x == self.selected)
            .unwrap_or(0);
        let next = visible[(pos as isize + d).clamp(0, visible.len() as isize - 1) as usize];
        if next != self.selected {
            self.selected_target = None;
        }
        self.selected = next;
    }
    fn target(&self) -> Option<String> {
        self.selected_target.clone().or_else(|| {
            self.packages.get(self.selected).and_then(|p| {
                if self.version_picker {
                    p.releases.get(p.release_index).map(|r| r.version.clone())
                } else if self.target_latest {
                    p.latest.clone().or(p.wanted.clone())
                } else {
                    p.wanted.clone().or(p.latest.clone())
                }
            })
        })
    }
    fn refresh(&mut self) {
        if self.loading || self.updating {
            return;
        }
        self.loading = true;
        self.status = "Loading dependencies…".into();
        let cwd = self.cwd.clone();
        let pm = self.pm;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Msg::Scan(
                scan(&cwd, pm).await.map_err(|e| format!("{e:#}")),
            ));
        });
    }
    fn update(&mut self) {
        let Some(p) = self.packages.get(self.selected).cloned() else {
            return;
        };
        let target = self.target().unwrap_or_else(|| p.current.clone());
        self.selected_target = None;
        let cwd = self.cwd.clone();
        let pm = self.pm;
        let upgrade = self.upgrade;
        let tx = self.tx.clone();
        self.confirm = false;
        self.updating = true;
        self.status = "Updating…".into();
        tokio::spawn(async move {
            let _ = tx.send(Msg::Update(update(&cwd, pm, &p, &target, upgrade).await));
        });
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.clear_cache {
        clear_cache()?;
        return Ok(());
    }
    let cwd = fs::canonicalize(&cli.cwd)
        .with_context(|| format!("Project directory not found: {}", cli.cwd.display()))?;
    let mf: Manifest = serde_json::from_slice(
        &fs::read(cwd.join("package.json")).context("Could not read package.json")?,
    )
    .context("package.json is invalid JSON")?;
    let pm = detect(cli.pm, mf.package_manager.as_deref(), &cwd)?;
    let total = mf.dependencies.as_ref().map_or(0, HashMap::len)
        + mf.dev.as_ref().map_or(0, HashMap::len)
        + mf.optional.as_ref().map_or(0, HashMap::len);
    let project = mf.name.unwrap_or_else(|| {
        cwd.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_owned()
    });
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App {
        cwd: cwd.clone(),
        project,
        pm,
        pm_version: "checking…".into(),
        total,
        packages: vec![],
        selected: 0,
        query: None,
        sort: Sort::Name,
        descending: false,
        loading: true,
        updating: false,
        confirm: false,
        version_picker: false,
        selected_target: None,
        target_latest: false,
        upgrade: false,
        status: "Loading dependencies…".into(),
        update_prompt: None,
        install_update: None,
        error: None,
        metadata_loading: HashMap::new(),
        metadata_requested: HashSet::new(),
        metadata_limit: Arc::new(tokio::sync::Semaphore::new(4)),
        tx: tx.clone(),
    };
    tokio::spawn(async move {
        let _ = tx.send(Msg::Scan(
            scan(&cwd, pm).await.map_err(|e| format!("{e:#}")),
        ));
    });
    let tx_version = app.tx.clone();
    tokio::spawn(async move {
        let version = Command::new(pm.name())
            .arg("--version")
            .output()
            .await
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| "N/A".into());
        let _ = tx_version.send(Msg::PmVersion(version));
    });
    if update_check_enabled() {
        app.update_prompt = read_update_cache()
            .as_ref()
            .and_then(pending_update)
            .map(|version| UpdatePrompt {
                version,
                selected: 0,
            });
        tokio::spawn(async {
            let _ = refresh_update_cache().await;
        });
    }
    let mut term = ratatui::init();
    let result = ui(&mut term, &mut app, &mut rx).await;
    ratatui::restore();
    result?;
    if let Some(version) = app.install_update {
        install_update(&version)?;
    }
    Ok(())
}

fn detect(explicit: Option<Pm>, field: Option<&str>, cwd: &Path) -> Result<Pm> {
    if let Some(pm) = explicit {
        return Ok(pm);
    }
    if let Some(name) = field.and_then(|x| x.split('@').next()) {
        return match name {
            "npm" => Ok(Pm::Npm),
            "pnpm" => Ok(Pm::Pnpm),
            "yarn" => Ok(Pm::Yarn),
            "bun" => Ok(Pm::Bun),
            _ => Err(anyhow!("Unsupported packageManager: {name}")),
        };
    }
    let locks = [
        ("package-lock.json", Pm::Npm),
        ("npm-shrinkwrap.json", Pm::Npm),
        ("pnpm-lock.yaml", Pm::Pnpm),
        ("yarn.lock", Pm::Yarn),
        ("bun.lock", Pm::Bun),
        ("bun.lockb", Pm::Bun),
    ];
    let found: Vec<_> = locks
        .into_iter()
        .filter(|(f, _)| cwd.join(f).exists())
        .collect();
    let pms: Vec<Pm> = found.iter().map(|(_, p)| *p).fold(vec![], |mut a, p| {
        if !a.contains(&p) {
            a.push(p)
        }
        a
    });
    match pms.as_slice() {
        [] => Ok(Pm::Npm),
        [p] => Ok(*p),
        _ => Err(anyhow!(
            "Ambiguous lockfiles ({}); choose one with --pm",
            found.iter().map(|x| x.0).collect::<Vec<_>>().join(", ")
        )),
    }
}

async fn scan(cwd: &Path, pm: Pm) -> Result<Vec<Package>> {
    let mf: Manifest = serde_json::from_slice(&fs::read(cwd.join("package.json"))?)?;
    let mut kinds = HashMap::new();
    for (kind, map) in [
        (Kind::Prod, mf.dependencies),
        (Kind::Dev, mf.dev),
        (Kind::Optional, mf.optional),
    ] {
        for (n, s) in map.unwrap_or_default() {
            kinds.insert(n, (s, kind));
        }
    }
    let mut cmd = Command::new(pm.name());
    cmd.current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match pm {
        Pm::Npm => {
            cmd.args(["outdated", "--json"]);
        }
        Pm::Pnpm => {
            cmd.args(["outdated", "--format", "json"]);
        }
        Pm::Yarn | Pm::Bun => {
            cmd.args(["outdated", "--json"]);
        }
    }
    let out = timeout(Duration::from_secs(45), cmd.output())
        .await
        .context("Timed out checking dependencies")??;
    if out.stdout.is_empty() && out.status.code().unwrap_or(0) > 1 {
        return Err(anyhow!(
            "{} outdated failed: {}",
            pm.name(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let data: Value = if pm == Pm::Yarn && !out.stdout.is_empty() {
        Value::Array(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect(),
        )
    } else if out.stdout.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&out.stdout).context("Invalid outdated JSON from package manager")?
    };
    let mut packages = vec![];
    for (name, current, wanted, latest) in parse_outdated(pm, &data)? {
        let Some((_, kind)) = kinds.get(&name) else {
            continue;
        };
        packages.push(Package {
            name,
            current: current.clone(),
            wanted,
            latest: latest.clone(),
            kind: *kind,
            change: change(&current, latest.as_deref()),
            description: None,
            license: None,
            repo: None,
            homepage: None,
            current_published: None,
            published: None,
            downloads: None,
            releases: vec![],
            release_index: 0,
        });
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}
fn parse_outdated(pm: Pm, v: &Value) -> Result<Vec<Outdated>> {
    let mut out = vec![];
    match pm {
        Pm::Npm | Pm::Bun => {
            if let Some(o) = v.as_object() {
                for (n, x) in o {
                    if let Some(x) = x.as_object() {
                        append_outdated(&mut out, n, x)
                    }
                }
            } else if let Some(a) = v.as_array() {
                for x in a {
                    if let Some(o) = x.as_object() {
                        if let Some(n) = field(o, &["name"]) {
                            append_outdated(&mut out, n, o)
                        }
                    }
                }
            }
        }
        Pm::Pnpm => {
            if let Some(object) = v.as_object() {
                for (name, item) in object {
                    if let Some(fields) = item.as_object() {
                        append_outdated(&mut out, name, fields);
                    }
                }
            } else if let Some(a) = v
                .as_array()
                .or_else(|| v.get("packages").and_then(Value::as_array))
            {
                for x in a {
                    if let Some(o) = x.as_object() {
                        if let Some(n) = field(o, &["name", "package"]) {
                            append_outdated(&mut out, n, o)
                        }
                    }
                }
            }
        }
        Pm::Yarn => {
            if let Some(a) = v.as_array() {
                for x in a {
                    let Some(o) = x.as_object() else { continue };
                    if let Some(name) = field(o, &["name"]) {
                        append_outdated(&mut out, name, o);
                    }
                    let Some(table) = x.pointer("/data").and_then(Value::as_object) else {
                        continue;
                    };
                    let heads = table.get("head").and_then(Value::as_array);
                    let body = table.get("body").and_then(Value::as_array);
                    let (Some(heads), Some(body)) = (heads, body) else {
                        continue;
                    };
                    let column = |labels: &[&str]| {
                        heads.iter().position(|h| {
                            h.as_str().is_some_and(|s| {
                                labels.iter().any(|label| s.eq_ignore_ascii_case(label))
                            })
                        })
                    };
                    let (Some(name_i), Some(current_i), Some(wanted_i), Some(latest_i)) = (
                        column(&["Package"]),
                        column(&["Current"]),
                        column(&["Wanted"]),
                        column(&["Latest"]),
                    ) else {
                        continue;
                    };
                    for row in body {
                        let Some(row) = row.as_array() else { continue };
                        let at = |i: usize| row.get(i).and_then(Value::as_str);
                        let Some(name) = at(name_i) else { continue };
                        out.push((
                            name.to_string(),
                            at(current_i).unwrap_or("N/A").to_string(),
                            at(wanted_i).map(str::to_string),
                            at(latest_i).map(str::to_string),
                        ));
                    }
                }
            }
        }
    }
    Ok(out)
}
fn append_outdated(out: &mut Vec<Outdated>, name: &str, fields: &serde_json::Map<String, Value>) {
    out.push((
        name.to_string(),
        field(fields, &["current"]).unwrap_or("N/A").to_string(),
        field(fields, &["wanted"]).map(str::to_string),
        field(fields, &["latest"]).map(str::to_string),
    ));
}
fn field<'a>(o: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| o.get(*k).and_then(Value::as_str))
}
fn change(a: &str, b: Option<&str>) -> Change {
    let (Ok(a), Some(Ok(b))) = (
        semver::Version::parse(clean_spec(a)),
        b.map(|s| semver::Version::parse(clean_spec(s))),
    ) else {
        return Change::Unknown;
    };
    if !b.pre.is_empty() {
        Change::Pre
    } else if a.major != b.major {
        Change::Major
    } else if a.minor != b.minor {
        Change::Minor
    } else if a.patch != b.patch {
        Change::Patch
    } else {
        Change::Unknown
    }
}
fn clean_spec(s: &str) -> &str {
    s.trim_start_matches(['v', '^', '~', '='])
}

fn cache_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|home| home.join(".cache"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        dirs::cache_dir()
    }
}

fn clear_cache() -> Result<()> {
    let path = cache_dir()
        .map(|dir| dir.join("depee"))
        .ok_or_else(|| anyhow!("Could not determine the platform cache directory"))?;
    match fs::remove_dir_all(&path) {
        Ok(()) => println!("Cleared depee cache at {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("No depee cache found at {}", path.display());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Could not clear cache at {}", path.display()))
        }
    }
    Ok(())
}

fn rank(c: Change) -> u8 {
    match c {
        Change::Major => 4,
        Change::Minor => 3,
        Change::Patch => 2,
        Change::Pre => 1,
        Change::Unknown => 0,
    }
}

async fn metadata(name: &str) -> Result<Metadata> {
    let client = reqwest::Client::builder()
        .user_agent("depee/0.1")
        .timeout(Duration::from_secs(12))
        .build()?;
    let safe = name.replace('/', "%2f");
    let url = format!("https://registry.npmjs.org/{}", safe);
    let cache = cache_dir().map(|d| {
        d.join("depee")
            .join(format!("{}.json", name.replace('/', "_").replace('@', "")))
    });
    let mut doc = None;
    if let Some(path) = &cache {
        if let Ok(bytes) = fs::read(path) {
            if let Ok((v, t)) = serde_json::from_slice::<(Value, DateTime<Utc>)>(&bytes) {
                if Utc::now().signed_duration_since(t).num_hours() < 12 {
                    doc = Some(v)
                }
            }
        }
    }
    let doc = match doc {
        Some(v) => v,
        None => {
            let v: Value = client
                .get(&url)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if let Some(path) = cache {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(path, serde_json::to_vec(&(v.clone(), Utc::now()))?);
            }
            v
        }
    };
    let latest = doc
        .pointer("/dist-tags/latest")
        .and_then(Value::as_str)
        .unwrap_or("");
    let versions = doc.get("versions").and_then(Value::as_object);
    let times = doc.get("time").and_then(Value::as_object);
    let mut releases: Vec<Release> = versions
        .into_iter()
        .flat_map(|v| v.iter())
        .map(|(version, data)| {
            let published = times
                .and_then(|t| t.get(version))
                .and_then(Value::as_str)
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc));
            let dependencies = data
                .get("dependencies")
                .and_then(Value::as_object)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            let prerelease = semver::Version::parse(version)
                .map(|v| !v.pre.is_empty())
                .unwrap_or(false);
            Release {
                version: version.clone(),
                published,
                change: Change::Unknown,
                prerelease,
                dependencies,
            }
        })
        .collect();
    releases.sort_by(|a, b| {
        semver::Version::parse(&b.version)
            .ok()
            .cmp(&semver::Version::parse(&a.version).ok())
    });
    let published = times
        .and_then(|t| t.get(latest))
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    let encoded = name.replace('/', "%2f");
    let downloads = match client
        .get(format!(
            "https://api.npmjs.org/downloads/point/last-week/{encoded}"
        ))
        .send()
        .await
    {
        Ok(response) => response
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v.get("downloads").and_then(Value::as_u64)),
        Err(_) => None,
    };
    let latest_data = versions.and_then(|v| v.get(latest));
    let repo = doc
        .pointer("/repository/url")
        .and_then(Value::as_str)
        .or_else(|| doc.get("repository").and_then(Value::as_str))
        .map(|s| {
            s.trim_start_matches("git+")
                .trim_end_matches(".git")
                .to_string()
        });
    Ok(Metadata {
        description: doc
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        license: latest_data
            .and_then(|v| v.get("license"))
            .and_then(|v| v.as_str().or_else(|| v.get("type").and_then(Value::as_str)))
            .map(str::to_string),
        repo,
        homepage: doc
            .get("homepage")
            .and_then(Value::as_str)
            .map(str::to_string),
        published,
        downloads,
        releases,
    })
}

async fn update(
    cwd: &Path,
    pm: Pm,
    p: &Package,
    target: &str,
    upgrade: bool,
) -> Result<String, String> {
    let args = update_args(pm, p, target, upgrade);
    let mut cmd = Command::new(pm.name());
    cmd.current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.args(args);
    let out = timeout(Duration::from_secs(600), cmd.output())
        .await
        .map_err(|_| "Update timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(format!("Updated {} to {}", p.name, target))
    } else {
        let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if e.is_empty() {
            format!("Update failed: {}", out.status)
        } else {
            e
        })
    }
}

fn update_args(pm: Pm, p: &Package, target: &str, upgrade: bool) -> Vec<String> {
    let selector = if upgrade {
        format!("{}@{}", p.name, target)
    } else {
        p.name.clone()
    };
    match (pm, upgrade, p.kind) {
        (Pm::Npm, false, _) | (Pm::Pnpm, false, _) | (Pm::Bun, false, _) => {
            vec!["update".into(), p.name.clone()]
        }
        (Pm::Yarn, false, _) => vec!["up".into(), p.name.clone()],
        (Pm::Npm, true, Kind::Dev) => vec!["install".into(), "--save-dev".into(), selector],
        (Pm::Npm, true, Kind::Optional) => {
            vec!["install".into(), "--save-optional".into(), selector]
        }
        (Pm::Npm, true, _) => vec!["install".into(), "--save".into(), selector],
        (Pm::Pnpm, true, Kind::Dev) => vec!["add".into(), "-D".into(), selector],
        (Pm::Pnpm, true, Kind::Optional) => vec!["add".into(), "-O".into(), selector],
        (Pm::Pnpm, true, _) => vec!["add".into(), selector],
        (Pm::Yarn, true, Kind::Dev) => vec!["add".into(), "-D".into(), selector],
        (Pm::Yarn, true, Kind::Optional) => vec!["add".into(), "-O".into(), selector],
        (Pm::Yarn, true, _) => vec!["add".into(), selector],
        (Pm::Bun, true, Kind::Dev) => vec!["add".into(), "-d".into(), selector],
        (Pm::Bun, true, Kind::Optional) => vec!["add".into(), "--optional".into(), selector],
        (Pm::Bun, true, _) => vec!["add".into(), selector],
    }
}

async fn ui(
    term: &mut DefaultTerminal,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
) -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let event_task = tokio::task::spawn_blocking(move || loop {
        if event_tx.is_closed() {
            break;
        }
        match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(event) => {
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }
                _ => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    });
    loop {
        term.draw(|f| draw(f, app))?;
        tokio::select! {
            Some(msg)=rx.recv()=>match msg{
                Msg::Scan(r)=>{app.loading=false;match r{Ok(p)=>{app.packages=p;app.metadata_loading.clear();app.metadata_requested.clear();app.error=None;app.status=format!("{} outdated",app.packages.len());if !app.packages.is_empty(){app.selected=app.selected.min(app.packages.len()-1);for index in 0..app.packages.len(){app.request_metadata_for(index);}}},Err(e)=>{app.status=e.clone();app.error=Some(e)}}},
                Msg::Metadata(i,r)=>{
                    let selected_name=app.packages.get(app.selected).map(|p|p.name.clone());
                    app.metadata_loading.insert(i,false);
                    if let(Some(p),Ok(m))=(app.packages.get_mut(i),r){
                        p.description=m.description;
                        p.license=m.license;
                        p.repo=m.repo;
                        p.homepage=m.homepage;
                        p.current_published=m.releases.iter().find(|r|r.version==p.current).and_then(|r|r.published);
                        p.published=m.published;
                        p.downloads=m.downloads;
                        p.releases=m.releases;
                        for x in &mut p.releases{x.change=change(&p.current,Some(&x.version));}
                    }
                    app.preserve_selection(selected_name);
                },
                Msg::PmVersion(version)=>app.pm_version=version,
                Msg::Update(r)=>{app.updating=false;match r{Ok(s)=>{app.status=s;app.error=None;app.refresh()},Err(e)=>{app.status="Update failed · press e for error".into();app.error=Some(e)}}}
            },
            Some(event)=event_rx.recv()=>if let Event::Key(k)=event{if key(app,k)?{break}}
        }
    }
    drop(event_rx);
    event_task
        .await
        .context("Terminal event reader task failed")?;
    Ok(())
}

fn update_cache_path() -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join("depee/.update-check.json"))
}

fn read_update_cache() -> Option<UpdateCheckCache> {
    let bytes = fs::read(update_cache_path()?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_update_cache(cache: &UpdateCheckCache) {
    let Some(path) = update_cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(cache) {
        let _ = fs::write(path, bytes);
    }
}

fn pending_update(cache: &UpdateCheckCache) -> Option<String> {
    cache
        .latest_version
        .clone()
        .filter(|version| is_newer(version))
        .filter(|version| cache.skipped_version.as_ref() != Some(version))
}

/// Fetches the latest release into the cache. The result is shown on the next launch.
async fn refresh_update_cache() -> Result<()> {
    const REPO: &str = "ruum0r/depee";
    const CACHE_TTL_HOURS: i64 = 24;

    if let Some(cache) = read_update_cache() {
        let age = Utc::now().signed_duration_since(cache.checked_at);
        if age.num_seconds() >= 0 && age.num_hours() < CACHE_TTL_HOURS {
            return Ok(());
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("depee/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(3))
        .build()?;
    let release: Value = client
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let latest_version = release
        .get("tag_name")
        .and_then(Value::as_str)
        .map(|tag| tag.trim_start_matches('v').to_string())
        .filter(|version| semver::Version::parse(version).is_ok());

    // Re-read so a version skipped while the request was in flight is kept.
    let skipped_version = read_update_cache().and_then(|cache| cache.skipped_version);
    write_update_cache(&UpdateCheckCache {
        checked_at: Utc::now(),
        latest_version,
        skipped_version,
    });
    Ok(())
}

fn skip_update(version: &str) {
    if let Some(mut cache) = read_update_cache() {
        cache.skipped_version = Some(version.to_string());
        write_update_cache(&cache);
    }
}

/// Replaces this process with the installer so the running binary can be overwritten.
#[cfg(unix)]
fn install_update(version: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe().context("Could not locate the depee executable")?;
    let install_dir = exe
        .parent()
        .ok_or_else(|| anyhow!("Could not determine the install directory"))?;
    println!("Updating depee to v{version}…");
    let error = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "curl -fsSL {INSTALL_SCRIPT_URL} | sh && echo 'Restart depee to use the new version.'"
        ))
        .env("DEPEE_INSTALL_DIR", install_dir)
        .exec();
    Err(error).context("Could not run the installer")
}

#[cfg(not(unix))]
fn install_update(_version: &str) -> Result<()> {
    println!("Install the update with: curl -fsSL {INSTALL_SCRIPT_URL} | sh");
    Ok(())
}

fn is_newer(version: &str) -> bool {
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"));
    let latest = semver::Version::parse(version);
    matches!((current, latest), (Ok(current), Ok(latest)) if latest > current)
}

fn update_check_enabled() -> bool {
    let Some(path) = dirs::home_dir().map(|home| home.join(".config/depee/config.toml")) else {
        return true;
    };
    let Ok(contents) = fs::read_to_string(path) else {
        return true;
    };

    contents
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next()?.trim();
            let (key, value) = line.split_once('=')?;
            (key.trim() == "check_for_updates").then(|| value.trim())
        })
        .last()
        .and_then(|value| match value {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
        .unwrap_or(true)
}

/// Up/down movement shared by every list: ↑/↓ and Ctrl+p/Ctrl+n, plus j/k when
/// letters are not being typed into a text field.
fn nav_delta(k: &KeyEvent, letters: bool) -> Option<isize> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Up => Some(-1),
        KeyCode::Down => Some(1),
        KeyCode::Char('p') if ctrl => Some(-1),
        KeyCode::Char('n') if ctrl => Some(1),
        KeyCode::Char('k') if letters && !ctrl => Some(-1),
        KeyCode::Char('j') if letters && !ctrl => Some(1),
        _ => None,
    }
}

fn step(index: usize, delta: isize, len: usize) -> usize {
    (index as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize
}

fn key(a: &mut App, k: KeyEvent) -> Result<bool> {
    // Ctrl+c quits from every screen, before any screen-specific handling.
    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(true);
    }
    if let Some(prompt) = &mut a.update_prompt {
        if let Some(d) = nav_delta(&k, true) {
            prompt.selected = step(prompt.selected, d, UPDATE_CHOICES.len());
            return Ok(false);
        }
        match k.code {
            KeyCode::Enter => {
                match prompt.selected {
                    0 => {
                        a.install_update = Some(prompt.version.clone());
                        return Ok(true);
                    }
                    1 => skip_update(&prompt.version),
                    _ => {}
                }
                a.update_prompt = None;
            }
            KeyCode::Esc => a.update_prompt = None,
            KeyCode::Char('q') => return Ok(true),
            _ => {}
        }
        return Ok(false);
    }
    if a.confirm {
        match k.code {
            KeyCode::Enter | KeyCode::Char('y') => a.update(),
            KeyCode::Char('m') => a.upgrade = !a.upgrade,
            KeyCode::Char('l') => a.target_latest = !a.target_latest,
            KeyCode::Esc | KeyCode::Char('n') => a.confirm = false,
            _ => {}
        }
        return Ok(false);
    }
    if a.version_picker {
        if let Some(d) = nav_delta(&k, true) {
            if let Some(p) = a.packages.get_mut(a.selected) {
                p.release_index = step(p.release_index, d, p.releases.len());
            }
            return Ok(false);
        }
        match k.code {
            KeyCode::Esc | KeyCode::Char('v') => a.version_picker = false,
            KeyCode::Enter => {
                a.selected_target = a
                    .packages
                    .get(a.selected)
                    .and_then(|p| p.releases.get(p.release_index))
                    .map(|r| r.version.clone());
                a.version_picker = false;
                a.upgrade = true;
                a.confirm = true
            }
            _ => {}
        }
        return Ok(false);
    }
    if a.query.is_some() {
        if let Some(d) = nav_delta(&k, false) {
            a.move_selection(d);
            return Ok(false);
        }
        match k.code {
            KeyCode::Esc | KeyCode::Enter => a.query = None,
            KeyCode::Backspace => {
                a.query.as_mut().unwrap().pop();
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                a.query.as_mut().unwrap().push(c);
                a.selected = a.visible().first().copied().unwrap_or(0)
            }
            _ => {}
        }
        return Ok(false);
    }
    if let Some(d) = nav_delta(&k, true) {
        a.move_selection(d);
        return Ok(false);
    }
    match k.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Enter => {
            a.request_metadata();
            if let Some(package) = a.packages.get(a.selected) {
                a.status = format!("Details: {}", package.name);
            }
        }
        KeyCode::Char('/') => a.query = Some(String::new()),
        KeyCode::Char('s') if !k.modifiers.contains(KeyModifiers::SHIFT) => {
            let selected_name = a.packages.get(a.selected).map(|p| p.name.clone());
            a.sort = match a.sort {
                Sort::Name => Sort::Change,
                Sort::Change => Sort::Published,
                Sort::Published => Sort::Name,
            };
            a.preserve_selection(selected_name);
        }
        KeyCode::Char('S') | KeyCode::Char('s') if k.modifiers.contains(KeyModifiers::SHIFT) => {
            let selected_name = a.packages.get(a.selected).map(|p| p.name.clone());
            a.descending = !a.descending;
            a.preserve_selection(selected_name);
        }
        KeyCode::Char('r') => a.refresh(),
        KeyCode::Char('u') => a.confirm = true,
        KeyCode::Char('v') => {
            a.selected_target = None;
            a.version_picker = true;
            a.request_metadata();
        }
        KeyCode::Char('c') => open_changelog(a)?,
        KeyCode::Char('e') => {
            if let Some(e) = &a.error {
                a.status = e.clone();
            }
        }
        KeyCode::Char('?') => {
            a.status = "↑↓/jk move · s cycle sort · S reverse · v version · u update · c changelog · r refresh · q quit".into()
        }
        _ => {}
    }
    Ok(false)
}
fn open_changelog(a: &mut App) -> Result<()> {
    let Some(p) = a.packages.get(a.selected) else {
        return Ok(());
    };
    let url = p
        .repo
        .as_deref()
        .filter(|x| x.contains("github.com"))
        .map(|x| format!("{}/releases", x.trim_end_matches('/')))
        .or_else(|| p.homepage.clone());
    if let Some(url) = url {
        let mut cmd = if cfg!(target_os = "macos") {
            Command::new("open")
        } else if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        } else {
            Command::new("xdg-open")
        };
        cmd.arg(url).spawn()?;
        a.status = "Opened package releases".into()
    } else {
        a.status = "No repository or homepage URL available".into()
    }
    Ok(())
}
fn draw(f: &mut Frame, a: &App) {
    let area = f.area();
    if area.width < 50 || area.height < 12 {
        f.render_widget(
            Paragraph::new("Terminal too small (minimum 50 x 12)")
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }
    if let Some(prompt) = &a.update_prompt {
        draw_update_prompt(f, prompt);
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);
    let (content_area, footer_area) = (rows[1], rows[2]);
    let state = if a.updating {
        "UPDATING"
    } else if a.loading {
        "LOADING"
    } else if a.error.is_some() {
        "ERROR"
    } else {
        "READY"
    };
    f.render_widget(
        Paragraph::new(format!(
            " depee · {} · {} {} · {} outdated / {} total · {} ",
            a.project,
            a.pm.name(),
            a.pm_version,
            a.packages.len(),
            a.total,
            state
        ))
        .block(Block::default().borders(Borders::ALL)),
        rows[0],
    );
    if area.width >= 120 {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(content_area);
        draw_list(f, a, panes[0]);
        draw_details(f, a, panes[1]);
    } else {
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(content_area);
        draw_list(f, a, panes[0]);
        draw_details(f, a, panes[1]);
    }
    let hints = if a.confirm {
        vec![
            Line::from("Enter confirm · l Wanted/Latest · m Compatible/Upgrade · Esc cancel"),
            Line::from("Review the package, target version, package manager, and command above."),
        ]
    } else if a.version_picker {
        vec![
            Line::from("↑/↓ or j/k select version · Enter choose · Esc cancel"),
            Line::from("Selected version is only applied after update confirmation."),
        ]
    } else if let Some(q) = &a.query {
        vec![
            Line::from(format!("Search: {q} · type to filter · Enter/Esc finish")),
            Line::from("↑/↓ or Ctrl+n/p moves through matching packages."),
        ]
    } else {
        vec![
            Line::from("↑↓ j/k Move · Enter Details · / Search · s Sort · v Versions · u Update"),
            Line::from(format!(
                "S Reverse · c Changelog · r Refresh · q Quit │ {}",
                a.status
            )),
        ]
    };
    f.render_widget(
        Paragraph::new(hints)
            .style(Style::default().fg(Color::Gray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .padding(Padding::horizontal(1)),
            ),
        footer_area,
    );
}
fn draw_update_prompt(f: &mut Frame, prompt: &UpdatePrompt) {
    let area = f.area();
    f.render_widget(
        Block::default().borders(Borders::ALL).title(" depee "),
        area,
    );
    let mut lines = vec![
        Line::from(Span::styled(
            "Update available",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!(
            "v{} → v{}",
            env!("CARGO_PKG_VERSION"),
            prompt.version
        )),
        Line::from(""),
    ];
    for (i, choice) in UPDATE_CHOICES.iter().enumerate() {
        lines.push(if i == prompt.selected {
            Line::from(Span::styled(
                format!("> {choice}"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(format!("  {choice}"))
        });
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ or j/k select · Enter confirm · Esc later · q quit",
        Style::default().fg(Color::DarkGray),
    )));
    let [body] = Layout::vertical([Constraint::Length(lines.len() as u16)])
        .flex(Flex::Center)
        .areas(area);
    let [body] = Layout::horizontal([Constraint::Length(48)])
        .flex(Flex::Center)
        .areas(body);
    f.render_widget(Paragraph::new(lines), body);
}
fn draw_list(f: &mut Frame, a: &App, area: ratatui::layout::Rect) {
    let ids = a.visible();
    let rows: Vec<Row> = ids
        .iter()
        .map(|i| {
            let p = &a.packages[*i];
            let (patch, minor, major) = version_lanes(p);
            Row::new(vec![
                Cell::from(p.name.clone()),
                Cell::from(short(&p.current).to_string()),
                Cell::from(patch).style(Style::default().fg(Color::Green)),
                Cell::from(minor).style(Style::default().fg(Color::Yellow)),
                Cell::from(major).style(Style::default().fg(Color::Red)),
                Cell::from(p.published.map(ago).unwrap_or_else(|| "N/A".into())),
            ])
        })
        .collect();
    let mut state = TableState::default();
    state.select(ids.iter().position(|i| *i == a.selected));
    let sort_name = match a.sort {
        Sort::Name => "Name",
        Sort::Change => "Change",
        Sort::Published => "Published",
    };
    let direction = if a.descending { "↓" } else { "↑" };
    let pending_metadata = a.metadata_loading.values().any(|pending| *pending);
    let using_name_while_loading = pending_metadata && matches!(a.sort, Sort::Published);
    let mut title = format!(" PACKAGES · sort: {sort_name} {direction}");
    if using_name_while_loading {
        title.push_str(" (active: Name)");
    }
    if let Some(query) = &a.query {
        title.push_str(&format!(" · filter: {query}"));
    }
    title.push(' ');
    f.render_stateful_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(19),
                Constraint::Percentage(17),
                Constraint::Percentage(14),
                Constraint::Percentage(14),
                Constraint::Percentage(14),
                Constraint::Percentage(22),
            ],
        )
        .header(
            Row::new([
                "Package",
                "Current",
                "Patch",
                "Minor",
                "Major",
                "Latest pub",
            ])
            .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .padding(Padding::horizontal(1))
                .title(title),
        )
        .row_highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::DarkGray),
        )
        .highlight_symbol("❯ "),
        area,
        &mut state,
    );
}
fn version_lanes(package: &Package) -> (String, String, String) {
    let Ok(current) = semver::Version::parse(clean_spec(&package.current)) else {
        return ("N/A".into(), "N/A".into(), "N/A".into());
    };
    let stable: Vec<(semver::Version, &str)> = package
        .releases
        .iter()
        .filter_map(|release| {
            let version = semver::Version::parse(&release.version).ok()?;
            version
                .pre
                .is_empty()
                .then_some((version, release.version.as_str()))
        })
        .collect();
    let newest = |mut matches: Vec<(semver::Version, &str)>| {
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        matches.last().map(|(_, version)| (*version).to_string())
    };
    let patch = newest(
        stable
            .iter()
            .filter(|(version, _)| {
                version.major == current.major
                    && version.minor == current.minor
                    && version >= &current
            })
            .map(|(version, text)| (version.clone(), *text))
            .collect(),
    )
    .unwrap_or_else(|| "N/A".into());
    let minor = newest(
        stable
            .iter()
            .filter(|(version, _)| version.major == current.major && version >= &current)
            .map(|(version, text)| (version.clone(), *text))
            .collect(),
    )
    .unwrap_or_else(|| patch.clone());
    let major = newest(stable)
        .or_else(|| package.latest.clone())
        .unwrap_or_else(|| "N/A".into());
    (patch, minor, major)
}
fn push_detail_section(lines: &mut Vec<Line<'static>>, title: &str) {
    lines.push(Line::from(Span::styled(
        format!(" {title}"),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
}

fn push_detail_field(lines: &mut Vec<Line<'static>>, label: &str, value: &str, width: usize) {
    let prefix = format!(" {label:<19}");
    let prefix_width = Span::raw(prefix.clone()).width();
    let value_width = width.saturating_sub(prefix_width + 1).max(1);
    let continuation = " ".repeat(prefix_width + 1);
    for (index, part) in wrap_detail_text(value, value_width).into_iter().enumerate() {
        if index == 0 {
            lines.push(Line::from(vec![
                Span::styled(prefix.clone(), Style::default().fg(Color::Gray)),
                Span::raw(" "),
                Span::raw(part),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::raw(continuation.clone()),
                Span::raw(part),
            ]));
        }
    }
}

fn push_detail_value(lines: &mut Vec<Line<'static>>, value: &str, width: usize) {
    let prefix = " ";
    let value_width = width.saturating_sub(prefix.len()).max(1);
    for part in wrap_detail_text(value, value_width) {
        lines.push(Line::from(vec![Span::raw(prefix), Span::raw(part)]));
    }
}

fn wrap_detail_text(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for word in value.split_whitespace() {
        let word_width = Span::raw(word).width();
        if current_width > 0 && current_width + 1 + word_width > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if current_width > 0 {
            current.push(' ');
            current_width += 1;
        }
        for character in word.chars() {
            let character = character.to_string();
            let character_width = Span::raw(character.clone()).width();
            if current_width > 0 && current_width + character_width > width {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push_str(&character);
            current_width += character_width;
        }
    }
    if current_width > 0 || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn draw_details(f: &mut Frame, a: &App, area: ratatui::layout::Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .title(if a.version_picker {
            " SELECT TARGET VERSION "
        } else {
            " PACKAGE DETAILS "
        });
    let Some(p) = a.packages.get(a.selected) else {
        f.render_widget(
            Paragraph::new(if a.loading {
                "Loading dependencies…"
            } else {
                "No outdated packages"
            })
            .block(block),
            area,
        );
        return;
    };
    if a.confirm {
        f.render_widget(
            Paragraph::new(vec![
                Line::from("Update package?"),
                Line::from(""),
                Line::from(format!("Package: {}", p.name)),
                Line::from(format!("Current: {}", p.current)),
                Line::from(format!(
                    "Target: {}",
                    a.target().unwrap_or_else(|| "N/A".into())
                )),
                Line::from(format!(
                    "Change: {}",
                    label(change(&p.current, a.target().as_deref()))
                )),
                Line::from(format!("Package manager: {}", a.pm.name())),
                Line::from(format!(
                    "Target source: {}",
                    if a.selected_target.is_some() {
                        "Selected version"
                    } else if a.target_latest {
                        "Latest"
                    } else {
                        "Wanted"
                    }
                )),
                Line::from(format!(
                    "Command: {} {}",
                    a.pm.name(),
                    update_args(
                        a.pm,
                        p,
                        &a.target().unwrap_or_else(|| p.current.clone()),
                        a.upgrade
                    )
                    .join(" ")
                )),
                Line::from(format!(
                    "Mode: {}",
                    if a.upgrade {
                        "Upgrade"
                    } else {
                        "Compatible Update"
                    }
                )),
            ])
            .block(block),
            area,
        );
        return;
    }
    if a.version_picker {
        let items: Vec<ListItem> = p
            .releases
            .iter()
            .enumerate()
            .map(|(i, r)| {
                ListItem::new(format!(
                    "{}  {}  {}{}",
                    r.version,
                    r.published.map(ago).unwrap_or("N/A".into()),
                    label(r.change),
                    if i == p.release_index {
                        "  ← selected"
                    } else {
                        ""
                    }
                ))
            })
            .collect();
        f.render_widget(List::new(items).block(block), area);
        return;
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner.width.max(1) as usize;
    let expanded_layout = inner.height >= 20;
    let mut lines = vec![Line::from(Span::styled(
        p.name.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    let wanted = p.wanted.as_deref().unwrap_or("N/A");
    let latest = p.latest.as_deref().unwrap_or("N/A");
    let current_published = p.current_published.map(ago).unwrap_or_else(|| "N/A".into());
    let latest_published = p.published.map(ago).unwrap_or_else(|| "N/A".into());
    if width >= 70 {
        if expanded_layout {
            lines.push(Line::from(""));
            push_detail_section(&mut lines, "VERSION");
            push_detail_field(&mut lines, "Current", &p.current, width);
            push_detail_field(&mut lines, "Wanted", wanted, width);
            push_detail_field(&mut lines, "Latest", latest, width);
            push_detail_field(&mut lines, "Change", label(p.change), width);
        } else {
            lines.push(Line::from(format!(
                " Current: {} · Wanted: {wanted}",
                p.current
            )));
            lines.push(Line::from(format!(
                " Latest: {latest} · Change: {}",
                label(p.change)
            )));
        }
    } else {
        push_detail_field(&mut lines, "Current", &p.current, width);
        push_detail_field(&mut lines, "Wanted", wanted, width);
        push_detail_field(&mut lines, "Latest", latest, width);
        push_detail_field(&mut lines, "Change", label(p.change), width);
        push_detail_field(&mut lines, "Current published", &current_published, width);
        push_detail_field(&mut lines, "Latest published", &latest_published, width);
    }
    let summary_height = lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>();
    if expanded_layout {
        lines.push(Line::from(""));
    }
    push_detail_section(&mut lines, "PUBLICATION");
    push_detail_field(&mut lines, "Current published", &current_published, width);
    push_detail_field(&mut lines, "Latest published", &latest_published, width);
    if expanded_layout {
        lines.push(Line::from(""));
    }
    push_detail_section(&mut lines, "PACKAGE");
    push_detail_field(
        &mut lines,
        "Downloads",
        &format!(
            "{} / week",
            p.downloads.map(count).unwrap_or_else(|| "N/A".into())
        ),
        width,
    );
    push_detail_field(
        &mut lines,
        "License",
        p.license.as_deref().unwrap_or("N/A"),
        width,
    );
    push_detail_field(
        &mut lines,
        "Repository",
        p.repo.as_deref().unwrap_or("N/A"),
        width,
    );
    push_detail_field(
        &mut lines,
        "Homepage",
        p.homepage.as_deref().unwrap_or("N/A"),
        width,
    );
    push_detail_field(
        &mut lines,
        "Description",
        p.description.as_deref().unwrap_or("N/A"),
        width,
    );
    if let Some(release) = p.releases.get(p.release_index) {
        let dependencies = if release.dependencies.is_empty() {
            "N/A".to_owned()
        } else {
            release.dependencies.join(", ")
        };
        if expanded_layout {
            lines.push(Line::from(""));
        }
        push_detail_section(
            &mut lines,
            &format!("TARGET DEPENDENCIES ({})", release.dependencies.len()),
        );
        push_detail_value(&mut lines, &dependencies, width);
    }
    let desired_releases = if inner.height >= 30 { 10 } else { 5 };
    let history_gap = u16::from(expanded_layout);
    let release_count = desired_releases.min(p.releases.len()).min(
        inner
            .height
            .saturating_sub(summary_height as u16 + history_gap + 2) as usize,
    );
    let history_reserve = if release_count > 0 {
        history_gap + 2 + release_count as u16
    } else {
        0
    };
    let info_height = lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>()
        .min(inner.height.saturating_sub(history_reserve) as usize) as u16;
    let info = Paragraph::new(lines);
    f.render_widget(
        info,
        ratatui::layout::Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: info_height,
        },
    );

    let history_y = inner.y + info_height + history_gap;
    let history_height = inner
        .height
        .saturating_sub(info_height)
        .min(2 + release_count as u16);
    if history_height > 0 {
        f.render_widget(
            Paragraph::new(" RELEASE HISTORY").style(Style::default().add_modifier(Modifier::BOLD)),
            ratatui::layout::Rect {
                x: inner.x,
                y: history_y,
                width: inner.width,
                height: 1,
            },
        );
    }
    if history_height > 1 {
        let rows = p.releases.iter().take(release_count).map(|release| {
            Row::new([
                release.version.clone(),
                release.published.map(ago).unwrap_or_else(|| "N/A".into()),
                format!(
                    "{}{}",
                    label(release.change),
                    if release.prerelease {
                        " · prerelease"
                    } else {
                        ""
                    }
                ),
            ])
        });
        f.render_widget(
            Table::new(
                rows,
                [
                    Constraint::Percentage(40),
                    Constraint::Percentage(30),
                    Constraint::Percentage(30),
                ],
            )
            .header(
                Row::new(["Version", "Published", "Change"])
                    .style(Style::default().add_modifier(Modifier::BOLD)),
            ),
            ratatui::layout::Rect {
                x: inner.x + 1,
                y: history_y + 1,
                width: inner.width.saturating_sub(1),
                height: history_height - 1,
            },
        );
    }
}
fn short(s: &str) -> &str {
    s.strip_prefix('^')
        .or_else(|| s.strip_prefix('~'))
        .unwrap_or(s)
}
fn label(c: Change) -> &'static str {
    match c {
        Change::Patch => "PATCH",
        Change::Minor => "MINOR",
        Change::Major => "MAJOR",
        Change::Pre => "PRE",
        Change::Unknown => "N/A",
    }
}
fn ago(d: DateTime<Utc>) -> String {
    let n = Utc::now().signed_duration_since(d).num_days();
    if n < 1 {
        "today".into()
    } else if n == 1 {
        "1 day ago".into()
    } else {
        format!("{n} days ago")
    }
}
fn count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests;
