//! Local dashboard discovery and loading, without UI or network dependencies.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use grafaui_model::Dashboard;
use serde_json::Value;

const BUILTINS: &[(&str, &str, &str)] = &[
    (
        "kitchen-sink",
        "kitchen-sink.json",
        include_str!("../../../fixtures/kitchen-sink.json"),
    ),
    (
        "legacy",
        "legacy.json",
        include_str!("../../../fixtures/legacy.json"),
    ),
    (
        "node-exporter",
        "node-exporter-full.json",
        include_str!("../../../fixtures/node-exporter-full.json"),
    ),
];

#[derive(Clone)]
enum Content {
    Embedded(String),
    File(PathBuf),
}

#[derive(Clone)]
/// A dashboard's stable identity and searchable local metadata.
pub struct DashboardEntry {
    id: String,
    title: String,
    origin: String,
    tags: Vec<String>,
    content: Content,
}

impl DashboardEntry {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    fn json(&self) -> Result<String, String> {
        match &self.content {
            Content::Embedded(json) => Ok(json.clone()),
            Content::File(path) => std::fs::read_to_string(path)
                .map_err(|e| format!("reading {}: {e}", path.display())),
        }
    }
}

#[derive(Clone)]
/// A startup snapshot of local dashboards; file contents are read when opened.
pub struct DashboardCatalog {
    entries: Vec<DashboardEntry>,
    warnings: Vec<String>,
}

impl DashboardCatalog {
    /// Embedded examples always remain available. By default, also index the
    /// fixtures beside the binary, falling back to the repository fixtures;
    /// a directory overrides that collection.
    pub fn discover(directory: Option<&Path>) -> Result<Self, String> {
        let mut catalog = Self {
            entries: Vec::new(),
            warnings: Vec::new(),
        };
        for (name, _, json) in BUILTINS {
            catalog.entries.push(entry(
                format!("fixture:{name}"),
                format!("fixture: {name}"),
                Content::Embedded((*json).into()),
                json,
            )?);
        }
        let fixtures = fixture_directory(std::env::current_exe().ok().as_deref());
        let root = directory.unwrap_or(&fixtures);
        if directory.is_some() && !root.is_dir() {
            return Err(format!(
                "dashboard directory does not exist: {}",
                root.display()
            ));
        }
        if root.is_dir() {
            let excluded: HashSet<_> = BUILTINS
                .iter()
                .filter_map(|(_, filename, _)| fixtures.join(filename).canonicalize().ok())
                .collect();
            catalog.scan(root, root, &excluded)?;
        }
        catalog.sort();
        Ok(catalog)
    }

    pub fn single(json: &str, origin: String) -> Result<Self, String> {
        Ok(Self {
            entries: vec![entry(
                "opened-dashboard".into(),
                origin,
                Content::Embedded(json.into()),
                json,
            )?],
            warnings: Vec::new(),
        })
    }

    pub fn entries(&self) -> &[DashboardEntry] {
        &self.entries
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn fixture_id(name: &str) -> String {
        format!("fixture:{name}")
    }

    pub fn find(&self, id: &str) -> Option<&DashboardEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Include a positional file in the picker, retaining path identity even if
    /// titles/UIDs collide. Opening reads the file again so edits are respected.
    pub fn include_file(&mut self, path: &Path) -> Result<String, String> {
        let path = path
            .canonicalize()
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let id = format!("file:{}", path.display());
        let json = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let origin = self.find(&id).map(|e| e.origin.clone()).unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let item = entry(id.clone(), origin, Content::File(path), &json)?;
        self.entries.retain(|e| e.id != id);
        self.entries.push(item);
        self.sort();
        Ok(id)
    }

    pub(crate) fn open(&self, id: &str, live: bool) -> Result<(Dashboard, String), String> {
        let entry = self
            .find(id)
            .ok_or("dashboard is no longer in the catalog")?;
        let json = entry.json()?;
        let dashboard = if live {
            Dashboard::parse_unexpanded(&json)
        } else {
            Dashboard::parse(&json)
        }
        .map_err(|e| format!("{}: {e}", entry.origin))?;
        if live
            && (grafaui_model::time::parse_relative(&dashboard.time_from).is_none()
                || dashboard.time_from.contains('/')
                || dashboard.time_to != "now")
        {
            return Err("Prometheus mode currently supports relative dashboard ranges ending at now; absolute and rounded ranges are unsupported".into());
        }
        Ok((dashboard, entry.origin.clone()))
    }

    fn sort(&mut self) {
        self.entries.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then(a.origin.cmp(&b.origin))
                .then(a.id.cmp(&b.id))
        });
    }

    fn scan(
        &mut self,
        directory: &Path,
        root: &Path,
        excluded: &HashSet<PathBuf>,
    ) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(directory)
            .map_err(|e| format!("reading {}: {e}", directory.display()))?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        entries.sort_by_key(|e| e.path());
        for item in entries {
            let kind = item.file_type().map_err(|e| e.to_string())?;
            let path = item.path();
            // Don't follow symlink directories and accidentally loop/outgrow the collection.
            if kind.is_dir() {
                self.scan(&path, root, excluded)?;
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            {
                if path
                    .canonicalize()
                    .ok()
                    .is_some_and(|p| excluded.contains(&p))
                {
                    continue;
                }
                let json = match std::fs::read_to_string(&path) {
                    Ok(json) => json,
                    Err(e) => {
                        self.warnings.push(format!("{}: {e}", path.display()));
                        continue;
                    }
                };
                match metadata(&json) {
                    Ok(Some(_)) => match self.include_file(&path) {
                        Ok(id) => {
                            if let Some(entry) =
                                self.entries.iter_mut().find(|entry| entry.id == id)
                            {
                                entry.origin = path
                                    .strip_prefix(root)
                                    .unwrap_or(&path)
                                    .display()
                                    .to_string();
                            }
                        }
                        Err(error) => self.warnings.push(error),
                    },
                    Ok(None) => {} // Source manifests and other JSON documents are not dashboards.
                    Err(error) => self.warnings.push(format!("{}: {error}", path.display())),
                }
            }
        }
        Ok(())
    }
}

fn fixture_directory(executable: Option<&Path>) -> PathBuf {
    if let Some(fixtures) = executable
        .and_then(Path::parent)
        .map(|parent| parent.join("fixtures"))
        .filter(|fixtures| fixtures.is_dir())
    {
        return fixtures;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn metadata(json: &str) -> Result<Option<(String, Vec<String>)>, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let value = value
        .get("dashboard")
        .filter(|v| v.is_object())
        .unwrap_or(&value);
    if !value.is_object()
        || !(value.get("panels").is_some_and(Value::is_array)
            || value.get("rows").is_some_and(Value::is_array))
    {
        return Ok(None);
    }
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("Untitled dashboard")
        .to_owned();
    let tags = value
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Ok(Some((title, tags)))
}

fn entry(
    id: String,
    origin: String,
    content: Content,
    json: &str,
) -> Result<DashboardEntry, String> {
    let (title, tags) = metadata(json)?
        .ok_or_else(|| format!("{origin}: JSON has no dashboard panels or legacy rows"))?;
    Ok(DashboardEntry {
        id,
        origin,
        title,
        tags,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "grafaui-catalog-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, path: &str, json: &str) -> PathBuf {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, json).unwrap();
            path
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn downloaded_binary_uses_adjacent_fixtures_instead_of_the_build_checkout() {
        let directory = Directory::new();
        let executable = directory.0.join("grafaui");
        assert_eq!(
            fixture_directory(Some(&executable)),
            fixture_directory(None)
        );
        directory.write(
            "fixtures/real/dashboard.json",
            r#"{"title":"Packaged","panels":[]}"#,
        );
        assert_eq!(
            fixture_directory(Some(&executable)),
            directory.0.join("fixtures")
        );
    }

    #[test]
    fn discovers_nested_wrapped_and_legacy_dashboards_without_manifests() {
        let directory = Directory::new();
        directory.write("z.JSON", r#"{"title":"Zulu","panels":[]}"#);
        directory.write(
            "nested/a.json",
            r#"{"dashboard":{"title":"Alpha","panels":[],"tags":["linux"]}}"#,
        );
        directory.write("old.json", r#"{"title":"Old","rows":[]}"#);
        directory.write("manifest.json", r#"{"dashboards":[]}"#);
        directory.write("bad.json", "broken");
        let catalog = DashboardCatalog::discover(Some(&directory.0)).unwrap();
        assert_eq!(catalog.entries().len(), 6);
        assert_eq!(catalog.entries()[0].title(), "Alpha");
        assert_eq!(catalog.entries()[0].tags(), ["linux"]);
        assert_eq!(catalog.warnings().len(), 1);
        assert!(catalog.warnings()[0].contains("bad.json"));
    }

    #[test]
    fn duplicate_titles_have_distinct_ids_and_files_are_deduplicated() {
        let directory = Directory::new();
        let first = directory.write("one/a.json", r#"{"title":"Same","panels":[]}"#);
        directory.write("two/a.json", r#"{"title":"Same","panels":[]}"#);
        let mut catalog = DashboardCatalog::discover(Some(&directory.0)).unwrap();
        let id = catalog.include_file(&first).unwrap();
        let matching: Vec<_> = catalog
            .entries()
            .iter()
            .filter(|e| e.title() == "Same")
            .collect();
        assert_eq!(matching.len(), 2);
        assert_ne!(matching[0].id(), matching[1].id());
        assert_ne!(matching[0].origin(), matching[1].origin());
        assert_eq!(catalog.open(&id, false).unwrap().0.title, "Same");
    }

    #[test]
    fn loading_reads_edits_and_reports_parse_and_live_range_errors() {
        let directory = Directory::new();
        let path = directory.write("editable.json", r#"{"title":"Before","panels":[]}"#);
        let mut catalog = DashboardCatalog::discover(Some(&directory.0)).unwrap();
        let id = catalog.include_file(&path).unwrap();
        std::fs::write(&path, r#"{"title":"After","panels":[]}"#).unwrap();
        assert_eq!(catalog.open(&id, true).unwrap().0.title, "After");
        std::fs::write(&path, "broken").unwrap();
        assert!(
            catalog
                .open(&id, false)
                .unwrap_err()
                .contains("editable.json")
        );
        let absolute = DashboardCatalog::single(
            r#"{"panels":[],"time":{"from":"2026-01-01","to":"now"}}"#,
            "absolute".into(),
        )
        .unwrap();
        assert!(absolute.open("opened-dashboard", true).is_err());
        assert!(absolute.open("opened-dashboard", false).is_ok());
    }
}
