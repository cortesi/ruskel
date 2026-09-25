//! Name resolution for API catalogues.
#![allow(
    clippy::redundant_pub_crate,
    reason = "capture and crateutils use these interfaces from sibling modules"
)]

use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fs,
    rc::Rc,
};

use rustdoc_types::{Crate, Id, ItemEnum, Path, Visibility};
use serde_json::Value;

use crate::{
    crateutils::render_generic_args,
    error::{Result, RuskelError},
    toolchain::toolchain_binary,
};

/// A definition identity that remains stable across rustdoc JSON documents.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct CrossCrateKey {
    /// Rust crate name from `external_crates` or the document root.
    crate_name: String,
    /// Definition path after the crate segment.
    path: Vec<String>,
}

/// Public paths exposed by one rustdoc document.
#[derive(Clone, Debug, Default)]
pub(crate) struct PublicPathIndex {
    /// Name of the crate whose document produced this index.
    crate_name: String,
    /// Public occurrences keyed by this document's IDs.
    by_id: BTreeMap<Id, Vec<String>>,
    /// Public occurrences keyed across documents.
    by_key: BTreeMap<CrossCrateKey, Vec<String>>,
    /// External modules re-exported by this document.
    module_reexports: BTreeMap<CrossCrateKey, Vec<String>>,
}

impl PublicPathIndex {
    /// Walk public modules and re-exports from the document root.
    pub(crate) fn build(crate_data: &Crate) -> Self {
        let crate_name = crate_data
            .index
            .get(&crate_data.root)
            .and_then(|root| root.name.clone())
            .or_else(|| {
                crate_data
                    .paths
                    .get(&crate_data.root)
                    .and_then(|summary| summary.path.first().cloned())
            })
            .unwrap_or_else(|| "crate".to_string());
        let mut index = Self {
            crate_name,
            ..Self::default()
        };
        let mut active = BTreeSet::new();
        index.walk_children(crate_data, crate_data.root, "crate", &mut active);
        for paths in index.by_id.values_mut() {
            sort_paths(paths);
        }
        for paths in index.by_key.values_mut() {
            sort_paths(paths);
        }
        index
    }

    /// Build an index from standard-library JSON without a rustdoc-types
    /// version match.
    fn build_raw(document: &Value, crate_name: &str) -> Result<Self> {
        let root = document
            .get("root")
            .and_then(Value::as_u64)
            .ok_or_else(|| RuskelError::Generate("standard-library JSON has no root ID".into()))?;
        let root_item = raw_item(document, root).ok_or_else(|| {
            RuskelError::Generate("standard-library JSON has no root module".into())
        })?;
        let root_crate_id = root_item
            .get("crate_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                RuskelError::Generate("standard-library JSON has no root crate ID".into())
            })?;
        let mut index = Self {
            crate_name: crate_name.into(),
            ..Self::default()
        };
        let mut active = BTreeSet::new();
        index.walk_raw_children(document, root, root_crate_id, "crate", &mut active);
        for paths in index.by_id.values_mut() {
            sort_paths(paths);
        }
        for paths in index.by_key.values_mut() {
            sort_paths(paths);
        }
        for paths in index.module_reexports.values_mut() {
            sort_paths(paths);
        }
        Ok(index)
    }

    /// Expand external module re-exports using their owning crate's index.
    fn graft_module_reexports(&mut self, origins: &[&Self]) {
        for (module_key, aliases) in &self.module_reexports {
            let Some(origin) = origins
                .iter()
                .find(|origin| origin.crate_name == module_key.crate_name)
            else {
                continue;
            };
            let Some(module_paths) = origin.by_key.get(module_key) else {
                continue;
            };
            for (definition, public_paths) in &origin.by_key {
                for public_path in public_paths {
                    for module_path in module_paths {
                        let Some(suffix) = public_path
                            .strip_prefix(module_path)
                            .and_then(|rest| rest.strip_prefix("::"))
                        else {
                            continue;
                        };
                        for alias in aliases {
                            self.by_key
                                .entry(definition.clone())
                                .or_default()
                                .push(format!("{alias}::{suffix}"));
                        }
                    }
                }
            }
        }
        for paths in self.by_key.values_mut() {
            sort_paths(paths);
        }
    }

    /// Return the shortest public path for an ID in this document.
    pub(crate) fn canonical_path(&self, id: Id) -> Option<&str> {
        self.public_paths(id)
            .and_then(|paths| paths.first())
            .map(String::as_str)
    }

    /// Return all public paths for an ID, shortest first, with lexical ties.
    pub(crate) fn public_paths(&self, id: Id) -> Option<&[String]> {
        self.by_id.get(&id).map(Vec::as_slice)
    }

    /// Return whether a public path reaches an ID.
    pub(crate) fn is_public(&self, id: Id) -> bool {
        self.by_id.contains_key(&id)
    }

    /// Find the public path of a definition from another rustdoc document.
    fn cross_crate_path(&self, key: &CrossCrateKey) -> Option<&str> {
        self.by_key
            .get(key)
            .and_then(|paths| paths.first())
            .map(String::as_str)
    }

    /// Walk one public module without following recursive re-exports forever.
    fn walk_children(
        &mut self,
        crate_data: &Crate,
        module_id: Id,
        prefix: &str,
        active: &mut BTreeSet<Id>,
    ) {
        if !active.insert(module_id) {
            return;
        }
        let Some(module) = crate_data.index.get(&module_id).and_then(|item| {
            if let ItemEnum::Module(module) = &item.inner {
                Some(module)
            } else {
                None
            }
        }) else {
            active.remove(&module_id);
            return;
        };
        for child_id in &module.items {
            let Some(item) = crate_data.index.get(child_id) else {
                continue;
            };
            if !matches!(item.visibility, Visibility::Public) {
                continue;
            }
            match &item.inner {
                ItemEnum::Use(import) => {
                    let Some(target_id) = import.id else {
                        continue;
                    };
                    if import.is_glob {
                        self.walk_glob(crate_data, target_id, prefix, active);
                    } else {
                        self.walk_item(
                            crate_data,
                            target_id,
                            &format!("{prefix}::{}", import.name),
                            active,
                        );
                    }
                }
                _ => {
                    if let Some(name) = &item.name {
                        self.walk_item(crate_data, *child_id, &format!("{prefix}::{name}"), active);
                    }
                }
            }
        }
        active.remove(&module_id);
    }

    /// Expand a public glob at its exported prefix.
    fn walk_glob(
        &mut self,
        crate_data: &Crate,
        module_id: Id,
        prefix: &str,
        active: &mut BTreeSet<Id>,
    ) {
        self.walk_children(crate_data, module_id, prefix, active);
    }

    /// Record one public occurrence and descend if it is a module.
    fn walk_item(&mut self, crate_data: &Crate, id: Id, path: &str, active: &mut BTreeSet<Id>) {
        self.by_id.entry(id).or_default().push(path.to_string());
        if let Some(key) = cross_crate_key(crate_data, id) {
            let external_path = path.replacen("crate", &self.crate_name, 1);
            self.by_key.entry(key).or_default().push(external_path);
        }
        if crate_data
            .index
            .get(&id)
            .is_some_and(|item| matches!(item.inner, ItemEnum::Module(_)))
        {
            self.walk_children(crate_data, id, path, active);
        }
    }

    /// Walk module and re-export records from a raw rustdoc JSON document.
    fn walk_raw_children(
        &mut self,
        document: &Value,
        module_id: u64,
        root_crate_id: u64,
        prefix: &str,
        active: &mut BTreeSet<u64>,
    ) {
        if !active.insert(module_id) {
            return;
        }
        let Some(children) = raw_item(document, module_id)
            .and_then(|item| item.get("inner"))
            .and_then(|inner| inner.get("module"))
            .and_then(|module| module.get("items"))
            .and_then(Value::as_array)
        else {
            active.remove(&module_id);
            return;
        };
        for child in children {
            let Some(child_id) = child.as_u64() else {
                continue;
            };
            let Some(item) = raw_item(document, child_id) else {
                continue;
            };
            if item.get("visibility").and_then(Value::as_str) != Some("public") {
                continue;
            }
            if let Some(import) = item.get("inner").and_then(|inner| inner.get("use")) {
                let Some(target_id) = import.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                if import.get("is_glob").and_then(Value::as_bool) == Some(true) {
                    if raw_item(document, target_id).is_some() {
                        self.walk_raw_children(document, target_id, root_crate_id, prefix, active);
                    } else if let Some(key) =
                        raw_cross_crate_key(document, target_id, root_crate_id, &self.crate_name)
                    {
                        self.module_reexports
                            .entry(key)
                            .or_default()
                            .push(prefix.replacen("crate", &self.crate_name, 1));
                    }
                } else if let Some(name) = import.get("name").and_then(Value::as_str) {
                    self.walk_raw_item(
                        document,
                        target_id,
                        root_crate_id,
                        &format!("{prefix}::{name}"),
                        active,
                    );
                }
            } else if let Some(name) = item.get("name").and_then(Value::as_str) {
                self.walk_raw_item(
                    document,
                    child_id,
                    root_crate_id,
                    &format!("{prefix}::{name}"),
                    active,
                );
            }
        }
        active.remove(&module_id);
    }

    /// Record one raw public occurrence and descend into a module target.
    fn walk_raw_item(
        &mut self,
        document: &Value,
        id: u64,
        root_crate_id: u64,
        path: &str,
        active: &mut BTreeSet<u64>,
    ) {
        if let Ok(id32) = u32::try_from(id) {
            self.by_id
                .entry(Id(id32))
                .or_default()
                .push(path.to_string());
        }
        if let Some(key) = raw_cross_crate_key(document, id, root_crate_id, &self.crate_name) {
            let public_path = path.replacen("crate", &self.crate_name, 1);
            if key.crate_name != self.crate_name
                && raw_summary(document, id)
                    .and_then(|summary| summary.get("kind"))
                    .and_then(Value::as_str)
                    == Some("module")
            {
                self.module_reexports
                    .entry(key.clone())
                    .or_default()
                    .push(public_path.clone());
            }
            self.by_key.entry(key).or_default().push(public_path);
        }
        if raw_item(document, id)
            .and_then(|item| item.get("inner"))
            .and_then(|inner| inner.get("module"))
            .is_some()
        {
            self.walk_raw_children(document, id, root_crate_id, path, active);
        }
    }
}

/// One captured member or standard-library document and its public paths.
#[derive(Clone, Debug)]
pub(crate) struct NamedCrate {
    /// Cargo package name for dependency lookup.
    pub(crate) package_name: String,
    /// Rust crate name for cross-document definition keys.
    pub(crate) crate_name: String,
    /// Public paths in this crate's rustdoc document.
    pub(crate) index: PublicPathIndex,
}

/// Build the standard-library public indexes from one capture toolchain.
pub(crate) fn load_standard_indexes(toolchain: &str) -> Result<Vec<NamedCrate>> {
    let rustc = toolchain_binary(toolchain, "rustc")?;
    let sysroot = rustc
        .parent()
        .and_then(|bin| bin.parent())
        .ok_or_else(|| RuskelError::Generate(format!("invalid rustc path: {}", rustc.display())))?;
    let json_dir = sysroot.join("share/doc/rust/json");
    let mut indexes = Vec::<NamedCrate>::new();
    for crate_name in ["core", "alloc", "std"] {
        let path = json_dir.join(format!("{crate_name}.json"));
        let contents = fs::read_to_string(&path).map_err(|error| {
            RuskelError::Generate(format!(
                "cannot read {crate_name} JSON for '{toolchain}' at '{}': {error}. Install rust-docs-json for this toolchain",
                path.display()
            ))
        })?;
        let document: Value = serde_json::from_str(&contents)?;
        let mut index = PublicPathIndex::build_raw(&document, crate_name)?;
        index.graft_module_reexports(
            &indexes
                .iter()
                .map(|origin| &origin.index)
                .collect::<Vec<_>>(),
        );
        indexes.push(NamedCrate {
            package_name: crate_name.to_string(),
            crate_name: crate_name.to_string(),
            index,
        });
    }
    indexes.sort_by_key(|index| match index.crate_name.as_str() {
        "std" => 0,
        "core" => 1,
        _ => 2,
    });
    Ok(indexes)
}

/// One resolved ID before short-name assignment.
#[derive(Clone, Debug)]
struct NameCandidate {
    /// Canonical public path or definition-path fallback.
    full_path: String,
    /// Whether the definition belongs to the rendered crate.
    local: bool,
    /// Whether the definition has a Rust prelude meaning.
    prelude: bool,
    /// Whether no public or dependency path reached the definition.
    fallback: bool,
    /// Direct dependency package exposed through the display path.
    exposed_package: Option<String>,
}

/// IDs observed during rendering and their assigned display names.
#[derive(Clone, Debug, Default)]
struct NameState {
    /// IDs used in a rendered declaration.
    used: BTreeSet<Id>,
    /// Final display path for each used ID.
    display: BTreeMap<Id, String>,
    /// Import tails grouped by leading crate name.
    imports: BTreeMap<String, BTreeSet<String>>,
    /// Whether the first rendering pass assigned short names.
    assigned: bool,
}

/// Paths, collisions, and imports for one catalogue file.
#[derive(Clone, Debug)]
pub(crate) struct NamePlan {
    /// Resolved candidates for rustdoc paths in the document.
    candidates: Rc<BTreeMap<Id, NameCandidate>>,
    /// Local definitions that reserve each short name.
    local_names: Rc<BTreeMap<String, BTreeSet<Id>>>,
    /// Shared state for both passes of this render.
    state: Rc<RefCell<NameState>>,
}

impl NamePlan {
    /// Prepare ID-based choices before rendering the file.
    pub(crate) fn new(
        crate_data: &Crate,
        local: &PublicPathIndex,
        members: &[NamedCrate],
        standard: &[NamedCrate],
        dependency_aliases: &BTreeMap<String, String>,
    ) -> Result<Self> {
        let written = written_paths(crate_data)?;
        let root_crate_id = crate_data
            .index
            .get(&crate_data.root)
            .map(|root| root.crate_id);
        let mut candidates = BTreeMap::new();
        for (id, paths) in written {
            let local_item = is_local(crate_data, id, root_crate_id);
            let definition = definition_path(crate_data, id);
            let mut candidate = if local_item {
                if local.is_public(id) {
                    NameCandidate {
                        full_path: local.canonical_path(id).unwrap_or("crate").to_string(),
                        local: true,
                        prelude: false,
                        fallback: false,
                        exposed_package: None,
                    }
                } else {
                    NameCandidate {
                        full_path: definition
                            .as_ref()
                            .map(|path| local_definition(path))
                            .or_else(|| paths.first().cloned())
                            .unwrap_or_else(|| "crate".to_string()),
                        local: true,
                        prelude: false,
                        fallback: true,
                        exposed_package: None,
                    }
                }
            } else {
                external_candidate(
                    crate_data,
                    id,
                    &paths,
                    definition.as_deref(),
                    members,
                    standard,
                    dependency_aliases,
                )
            };
            if !local_item
                && let Some(root) = candidate.full_path.split("::").next()
                && !dependency_aliases.contains_key(root)
                && let Some(package) = external_package(crate_data, root)
            {
                candidate.exposed_package = Some(package);
            }
            candidates.insert(id, candidate);
        }
        let mut local_names = BTreeMap::<String, BTreeSet<Id>>::new();
        for (id, paths) in &local.by_id {
            if !is_local(crate_data, *id, root_crate_id) {
                continue;
            }
            if let Some(path) = paths.first()
                && let Some(short) = last_segment(path)
            {
                local_names
                    .entry(short.to_string())
                    .or_default()
                    .insert(*id);
            }
        }
        Ok(Self {
            candidates: Rc::new(candidates),
            local_names: Rc::new(local_names),
            state: Rc::new(RefCell::new(NameState::default())),
        })
    }

    /// Collect rendered IDs, assign names, then render with final names.
    pub(crate) fn render(&self, mut source: impl FnMut() -> Result<String>) -> Result<String> {
        with_names(self, &mut source)?;
        self.assign_short_names();
        with_names(self, source)
    }

    /// Resolve the base of a path and record its use in the file.
    pub(crate) fn base_name(&self, path: &Path) -> Option<String> {
        let candidate = self.candidates.get(&path.id)?;
        let mut state = self.state.borrow_mut();
        state.used.insert(path.id);
        Some(
            state
                .display
                .get(&path.id)
                .cloned()
                .unwrap_or_else(|| candidate.full_path.clone()),
        )
    }

    /// Resolve a complete path, including generic arguments.
    pub(crate) fn render_path(&self, path: &Path) -> String {
        let base = self
            .base_name(path)
            .unwrap_or_else(|| path.path.replace("$crate::", ""));
        let args = path
            .args
            .as_ref()
            .map(|args| render_generic_args(args))
            .unwrap_or_default();
        format!("{base}{args}")
    }

    /// Imports for names actually rendered in this file.
    pub(crate) fn use_block(&self) -> String {
        let state = self.state.borrow();
        if !state.assigned {
            return String::new();
        }
        let mut output = String::new();
        let mut crates = state.imports.keys().collect::<Vec<_>>();
        crates.sort_by_key(|name| (name.as_str() != "std", name.as_str()));
        for crate_name in crates {
            let paths = &state.imports[crate_name];
            if paths.len() == 1 {
                output.push_str(&format!(
                    "use {}::{};\n",
                    crate_name,
                    paths.first().unwrap()
                ));
            } else {
                output.push_str(&format!(
                    "use {}::{{{}}};\n",
                    crate_name,
                    paths.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output
    }

    /// Number of displayed names that require definition-path fallback.
    pub(crate) fn definition_fallbacks(&self) -> usize {
        let state = self.state.borrow();
        state
            .used
            .iter()
            .filter(|id| self.candidates.get(id).is_some_and(|name| name.fallback))
            .count()
    }

    /// Dependency packages named by displayed paths.
    pub(crate) fn exposed_packages(&self) -> Vec<String> {
        let state = self.state.borrow();
        state
            .used
            .iter()
            .filter_map(|id| self.candidates.get(id)?.exposed_package.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Local definitions used in signatures but unreachable through public
    /// paths.
    pub(crate) fn unnameable(&self) -> Vec<String> {
        let state = self.state.borrow();
        state
            .used
            .iter()
            .filter_map(|id| {
                let name = self.candidates.get(id)?;
                (name.local && name.fallback).then(|| name.full_path.clone())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Assign imports and display paths for the IDs recorded in the first pass.
    fn assign_short_names(&self) {
        let mut state = self.state.borrow_mut();
        let mut short_to_ids = BTreeMap::<String, BTreeSet<Id>>::new();
        for id in &state.used {
            let Some(name) = self.candidates.get(id) else {
                continue;
            };
            if !name.local
                && !name.prelude
                && let Some(short) = last_segment(&name.full_path)
            {
                short_to_ids
                    .entry(short.to_string())
                    .or_default()
                    .insert(*id);
            }
        }
        let used_ids = state.used.clone();
        for id in used_ids {
            let Some(name) = self.candidates.get(&id) else {
                continue;
            };
            let Some(short) = last_segment(&name.full_path) else {
                continue;
            };
            let unique_local = self
                .local_names
                .get(short)
                .is_some_and(|ids| ids.len() == 1);
            let local_collision = self.local_names.contains_key(short);
            let external_collision = short_to_ids.get(short).is_some_and(|ids| ids.len() > 1);
            let display = if name.prelude || (name.local && unique_local && !is_prelude_name(short))
            {
                short.to_string()
            } else if !name.local
                && !local_collision
                && !external_collision
                && !is_prelude_name(short)
            {
                if let Some((crate_name, rest)) = name.full_path.split_once("::") {
                    state
                        .imports
                        .entry(crate_name.to_string())
                        .or_default()
                        .insert(rest.to_string());
                }
                short.to_string()
            } else {
                name.full_path.clone()
            };
            state.display.insert(id, display);
        }
        state.assigned = true;
    }
}

thread_local! {
    static ACTIVE_NAMES: RefCell<Vec<NamePlan>> = const { RefCell::new(Vec::new()) };
}

/// Use a plan for the current render and restore the previous plan on exit.
fn with_names<T>(plan: &NamePlan, action: impl FnOnce() -> T) -> T {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            ACTIVE_NAMES.with(|active| {
                active.borrow_mut().pop();
            });
        }
    }
    ACTIVE_NAMES.with(|active| active.borrow_mut().push(plan.clone()));
    let guard = Guard;
    let result = action();
    drop(guard);
    result
}

/// Resolve one path from crateutils when a catalogue render is active.
pub(crate) fn active_base_name(path: &Path) -> Option<String> {
    ACTIVE_NAMES.with(|active| active.borrow().last().and_then(|plan| plan.base_name(path)))
}

/// Apply the member, standard-library, and written-path resolution rules.
fn external_candidate(
    crate_data: &Crate,
    id: Id,
    written: &[String],
    definition: Option<&[String]>,
    members: &[NamedCrate],
    standard: &[NamedCrate],
    dependency_aliases: &BTreeMap<String, String>,
) -> NameCandidate {
    let key = cross_crate_key(crate_data, id);
    if let Some(key) = &key {
        for member in members {
            if !dependency_aliases
                .values()
                .any(|package| package == &member.package_name)
            {
                continue;
            }
            let normalized_key = CrossCrateKey {
                crate_name: member.crate_name.clone(),
                path: key.path.clone(),
            };
            let path = member.index.cross_crate_path(key).or_else(|| {
                if key.crate_name == member.crate_name
                    || dependency_aliases.get(&key.crate_name) == Some(&member.package_name)
                {
                    member.index.cross_crate_path(&normalized_key)
                } else {
                    None
                }
            });
            if let Some(path) = path {
                let alias = dependency_aliases
                    .iter()
                    .filter(|(_, package)| *package == &member.package_name)
                    .map(|(alias, _)| alias.as_str())
                    .min()
                    .unwrap_or(&member.crate_name);
                let suffix = path.split_once("::").map(|(_, tail)| tail).unwrap_or("");
                return NameCandidate::external(
                    format!("{alias}::{suffix}"),
                    false,
                    dependency_aliases,
                );
            }
        }
        if matches!(key.crate_name.as_str(), "std" | "core" | "alloc") {
            for crate_name in ["std", "core", "alloc"] {
                let Some(index) = standard.iter().find(|index| index.crate_name == crate_name)
                else {
                    continue;
                };
                if let Some(path) = index.index.cross_crate_path(key) {
                    let prelude = is_prelude_definition(key);
                    return NameCandidate::external(path.to_string(), false, dependency_aliases)
                        .with_prelude(prelude);
                }
            }
        }
    }
    let direct_path = written
        .iter()
        .filter(|path| {
            path.split("::")
                .next()
                .is_some_and(|root| dependency_aliases.contains_key(root))
        })
        .min_by(|a, b| path_order(a, b));
    if let Some(path) = direct_path {
        return NameCandidate::external(path.clone(), false, dependency_aliases);
    }
    let fallback = definition
        .map(|path| path.join("::"))
        .or_else(|| written.first().cloned())
        .unwrap_or_default();
    NameCandidate::external(fallback, true, dependency_aliases)
        .with_prelude(key.as_ref().is_some_and(is_prelude_definition))
}

impl NameCandidate {
    /// Build one external candidate and identify its direct dependency package.
    fn external(
        full_path: String,
        fallback: bool,
        dependency_aliases: &BTreeMap<String, String>,
    ) -> Self {
        let exposed_package = full_path
            .split("::")
            .next()
            .filter(|root| !matches!(*root, "crate" | "std" | "core" | "alloc"))
            .map(|root| {
                dependency_aliases
                    .get(root)
                    .cloned()
                    .unwrap_or_else(|| root.to_string())
            });
        Self {
            full_path,
            local: false,
            prelude: false,
            fallback,
            exposed_package,
        }
    }

    /// Preserve whether this standard item is defined in the prelude.
    fn with_prelude(mut self, prelude: bool) -> Self {
        self.prelude = prelude;
        self
    }
}

/// Recover the Cargo package name of a transitive crate in a fallback path.
fn external_package(crate_data: &Crate, crate_name: &str) -> Option<String> {
    let external = crate_data
        .external_crates
        .values()
        .find(|external| external.name == crate_name)?;
    if let Some(package) = external
        .html_root_url
        .as_deref()
        .and_then(|url| url.strip_prefix("https://docs.rs/"))
        .and_then(|tail| tail.split('/').next())
        .filter(|package| !package.is_empty())
    {
        return Some(package.to_string());
    }
    let components = external
        .path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    components
        .windows(2)
        .rev()
        .find_map(|pair| (pair[0] == "build").then(|| pair[1].to_string()))
}

/// Collect all written paths for each ID, independent of hash-map order.
fn written_paths(crate_data: &Crate) -> Result<BTreeMap<Id, Vec<String>>> {
    let value = serde_json::to_value(crate_data)?;
    let mut paths = BTreeMap::<Id, Vec<String>>::new();
    visit_written_paths(&value, &mut paths);
    for paths in paths.values_mut() {
        sort_paths(paths);
    }
    Ok(paths)
}

/// Visit serialized rustdoc values to find `Path { path, id, args }` objects.
fn visit_written_paths(value: &Value, paths: &mut BTreeMap<Id, Vec<String>>) {
    match value {
        Value::Object(object) => {
            if let (Some(Value::String(path)), Some(Value::Number(number))) =
                (object.get("path"), object.get("id"))
                && let Some(id) = number.as_u64().and_then(|id| u32::try_from(id).ok())
            {
                paths.entry(Id(id)).or_default().push(path.clone());
            }
            for value in object.values() {
                visit_written_paths(value, paths);
            }
        }
        Value::Array(array) => {
            for value in array {
                visit_written_paths(value, paths);
            }
        }
        _ => {}
    }
}

/// Turn a document-local ID into a definition identity across documents.
fn cross_crate_key(crate_data: &Crate, id: Id) -> Option<CrossCrateKey> {
    let summary = crate_data.paths.get(&id)?;
    let root_crate_id = crate_data.index.get(&crate_data.root)?.crate_id;
    let crate_name = if summary.crate_id == root_crate_id {
        crate_data.index.get(&crate_data.root)?.name.clone()?
    } else {
        crate_data
            .external_crates
            .get(&summary.crate_id)?
            .name
            .clone()
    };
    Some(CrossCrateKey {
        crate_name,
        path: summary.path.iter().skip(1).cloned().collect(),
    })
}

/// Find one item by its numeric ID in a raw rustdoc document.
fn raw_item(document: &Value, id: u64) -> Option<&Value> {
    document.get("index")?.get(id.to_string().as_str())
}

/// Find one path summary by its numeric ID in a raw rustdoc document.
fn raw_summary(document: &Value, id: u64) -> Option<&Value> {
    document.get("paths")?.get(id.to_string().as_str())
}

/// Build a definition key from raw standard-library path metadata.
fn raw_cross_crate_key(
    document: &Value,
    id: u64,
    root_crate_id: u64,
    root_crate_name: &str,
) -> Option<CrossCrateKey> {
    let summary = raw_summary(document, id)?;
    let crate_id = summary.get("crate_id")?.as_u64()?;
    let crate_name = if crate_id == root_crate_id {
        root_crate_name.to_string()
    } else {
        document
            .get("external_crates")?
            .get(crate_id.to_string().as_str())?
            .get("name")?
            .as_str()?
            .to_string()
    };
    let path = summary
        .get("path")?
        .as_array()?
        .iter()
        .skip(1)
        .map(|segment| segment.as_str().map(ToString::to_string))
        .collect::<Option<Vec<_>>>()?;
    Some(CrossCrateKey { crate_name, path })
}

/// Return whether an ID belongs to the document's root crate.
fn is_local(crate_data: &Crate, id: Id, root_crate_id: Option<u32>) -> bool {
    let item_crate = crate_data
        .paths
        .get(&id)
        .map(|summary| summary.crate_id)
        .or_else(|| crate_data.index.get(&id).map(|item| item.crate_id));
    item_crate.is_some() && item_crate == root_crate_id
}

/// Return rustdoc's definition path for one ID.
fn definition_path(crate_data: &Crate, id: Id) -> Option<Vec<String>> {
    crate_data
        .paths
        .get(&id)
        .map(|summary| summary.path.clone())
}

/// Display a local definition path with the `crate::` prefix.
fn local_definition(path: &[String]) -> String {
    if path.len() < 2 {
        "crate".to_string()
    } else {
        format!("crate::{}", path[1..].join("::"))
    }
}

/// Sort and deduplicate paths by segment count and then lexical order.
fn sort_paths(paths: &mut Vec<String>) {
    paths.sort_by(|a, b| path_order(a, b));
    paths.dedup();
}

/// Compare public or written paths with the catalogue's total order.
fn path_order(a: &str, b: &str) -> Ordering {
    a.split("::")
        .count()
        .cmp(&b.split("::").count())
        .then_with(|| a.cmp(b))
}

/// Return the short name of a nonempty path.
fn last_segment(path: &str) -> Option<&str> {
    path.rsplit("::")
        .next()
        .filter(|segment| !segment.is_empty())
}

/// Reserve Rust 2024 prelude spellings for their standard meanings.
fn is_prelude_name(name: &str) -> bool {
    matches!(
        name,
        "AsMut"
            | "AsRef"
            | "Box"
            | "Clone"
            | "Copy"
            | "Default"
            | "DoubleEndedIterator"
            | "Drop"
            | "Eq"
            | "Err"
            | "ExactSizeIterator"
            | "Extend"
            | "Fn"
            | "FnMut"
            | "FnOnce"
            | "From"
            | "FromIterator"
            | "Future"
            | "Into"
            | "IntoFuture"
            | "IntoIterator"
            | "Iterator"
            | "None"
            | "Ok"
            | "Option"
            | "Ord"
            | "PartialEq"
            | "PartialOrd"
            | "Result"
            | "Send"
            | "Sized"
            | "Some"
            | "String"
            | "Sync"
            | "ToOwned"
            | "ToString"
            | "TryFrom"
            | "TryInto"
            | "Vec"
    )
}

/// Identify a standard definition that the Rust 2024 prelude imports.
fn is_prelude_definition(key: &CrossCrateKey) -> bool {
    let (Some(family), Some(item)) = (key.path.first(), key.path.last()) else {
        return false;
    };
    if key.path.len() < 2 {
        return false;
    }
    matches!(
        (key.crate_name.as_str(), family.as_str(), item.as_str()),
        ("alloc", "boxed", "Box")
            | ("alloc", "string", "String" | "ToString")
            | ("alloc", "vec", "Vec")
            | ("alloc", "borrow", "ToOwned")
            | ("core", "clone", "Clone")
            | ("core", "marker", "Copy" | "Send" | "Sized" | "Sync")
            | ("core", "default", "Default")
            | ("core", "cmp", "Eq" | "Ord" | "PartialEq" | "PartialOrd")
            | (
                "core",
                "convert",
                "AsMut" | "AsRef" | "From" | "Into" | "TryFrom" | "TryInto"
            )
            | ("core", "future", "Future" | "IntoFuture")
            | (
                "core",
                "iter",
                "Iterator"
                    | "IntoIterator"
                    | "FromIterator"
                    | "Extend"
                    | "DoubleEndedIterator"
                    | "ExactSizeIterator"
            )
            | ("core", "ops", "Drop" | "Fn" | "FnMut" | "FnOnce")
            | ("core", "option", "Option")
            | ("core", "result", "Result")
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use rustdoc_types::{
        ExternalCrate, Generics, Item, ItemEnum, ItemKind, ItemSummary, Module, Target, Type,
        TypeAlias, Use,
    };

    use super::*;
    use crate::crateutils::render_path;

    #[test]
    fn prelude_definitions_allow_private_modules_within_known_families() {
        let key = |crate_name: &str, path: &[&str]| CrossCrateKey {
            crate_name: crate_name.into(),
            path: path.iter().map(|segment| (*segment).into()).collect(),
        };
        let prelude: &[(&str, &[&str])] = &[
            ("core", &["future", "future", "Future"]),
            ("core", &["future", "into_future", "IntoFuture"]),
            ("core", &["iter", "traits", "iterator", "Iterator"]),
            ("core", &["iter", "traits", "collect", "IntoIterator"]),
            ("core", &["ops", "function", "FnOnce"]),
            ("alloc", &["string", "ToString"]),
        ];
        for &(crate_name, path) in prelude {
            assert!(
                is_prelude_definition(&key(crate_name, path)),
                "{crate_name}::{path:?}"
            );
        }
        let non_prelude: &[(&str, &[&str])] = &[
            ("core", &["fmt", "Result"]),
            ("core", &["io", "error", "Result"]),
            ("std", &["future", "future", "Future"]),
            ("core", &["future", "FutureLike"]),
        ];
        for &(crate_name, path) in non_prelude {
            assert!(
                !is_prelude_definition(&key(crate_name, path)),
                "{crate_name}::{path:?}"
            );
        }
    }

    #[test]
    fn derived_error_trait_stays_qualified_beside_local_error() -> Result<()> {
        let mut data = empty_crate("fixture", &[1, 2]);
        data.index.insert(
            Id(1),
            item(1, "Error", Visibility::Public, module(&[], false)),
        );
        data.index.insert(
            Id(2),
            item(
                2,
                "Derived",
                Visibility::Public,
                ItemEnum::TypeAlias(TypeAlias {
                    type_: Type::ResolvedPath(path(10, "Error")),
                    generics: Generics {
                        params: Vec::new(),
                        where_predicates: Vec::new(),
                    },
                }),
            ),
        );
        data.paths
            .insert(Id(1), summary(0, &["fixture", "Error"], ItemKind::Module));
        data.paths.insert(
            Id(10),
            summary(1, &["core", "error", "Error"], ItemKind::Trait),
        );
        data.external_crates.insert(
            1,
            ExternalCrate {
                name: "core".into(),
                html_root_url: None,
                path: PathBuf::new(),
            },
        );
        let mut std_data = empty_crate("std", &[1]);
        std_data.index.insert(
            Id(1),
            item(1, "error", Visibility::Public, module(&[2], false)),
        );
        add_alias(&mut std_data, 2, "Error", 50, false);
        std_data.paths.insert(
            Id(50),
            summary(1, &["core", "error", "Error"], ItemKind::Trait),
        );
        std_data.external_crates.insert(
            1,
            ExternalCrate {
                name: "core".into(),
                html_root_url: None,
                path: PathBuf::new(),
            },
        );
        let standard = NamedCrate {
            package_name: "std".into(),
            crate_name: "std".into(),
            index: PublicPathIndex::build(&std_data),
        };
        let local = PublicPathIndex::build(&data);
        let plan = NamePlan::new(&data, &local, &[], &[standard], &BTreeMap::new())?;
        let output = plan.render(|| Ok(plan.render_path(&path(10, "Error"))))?;
        assert_eq!(output, "std::error::Error");
        assert!(plan.use_block().is_empty());
        Ok(())
    }

    #[test]
    fn standard_index_reads_format_60_modules_and_reexports() -> Result<()> {
        let document = serde_json::json!({
            "format_version": 60,
            "root": 1,
            "index": {
                "1": {"crate_id": 0, "name": "std", "visibility": "public", "inner": {"module": {"items": [2, 3, 4, 7]}}},
                "2": {"crate_id": 0, "name": "collections", "visibility": "public", "inner": {"module": {"items": [5]}}},
                "3": {"crate_id": 0, "name": "Vec", "visibility": "public", "inner": {"use": {"source": "alloc::vec::Vec", "name": "Vec", "id": 90, "is_glob": false}}},
                "4": {"crate_id": 0, "name": "hidden", "visibility": "default", "inner": {"module": {"items": [6]}}},
                "5": {"crate_id": 0, "name": "HashMap", "visibility": "public", "inner": {"use": {"source": "hashbrown::HashMap", "name": "HashMap", "id": 91, "is_glob": false}}},
                "6": {"crate_id": 0, "name": "Invisible", "visibility": "public", "inner": {"module": {"items": []}}},
                "7": {"crate_id": 0, "name": null, "visibility": "public", "inner": {"use": {"source": "alloc::vec", "name": "vec", "id": 92, "is_glob": false}}}
            },
            "paths": {
                "90": {"crate_id": 1, "path": ["alloc", "vec", "Vec"], "kind": "struct"},
                "91": {"crate_id": 2, "path": ["hashbrown", "map", "HashMap"], "kind": "struct"},
                "92": {"crate_id": 1, "path": ["alloc", "vec"], "kind": "module"}
            },
            "external_crates": {
                "1": {"name": "alloc"},
                "2": {"name": "hashbrown"}
            }
        });
        let origin = serde_json::json!({
            "format_version": 60,
            "root": 1,
            "index": {
                "1": {"crate_id": 0, "name": "alloc", "visibility": "public", "inner": {"module": {"items": [2]}}},
                "2": {"crate_id": 0, "name": "vec", "visibility": "public", "inner": {"module": {"items": [3]}}},
                "3": {"crate_id": 0, "name": "Vec", "visibility": "public", "inner": {"struct": {}}}
            },
            "paths": {
                "2": {"crate_id": 0, "path": ["alloc", "vec"], "kind": "module"},
                "3": {"crate_id": 0, "path": ["alloc", "vec", "Vec"], "kind": "struct"}
            },
            "external_crates": {}
        });
        let alloc = PublicPathIndex::build_raw(&origin, "alloc")?;
        let mut index = PublicPathIndex::build_raw(&document, "std")?;
        index.graft_module_reexports(&[&alloc]);
        assert_eq!(index.canonical_path(Id(90)), Some("crate::Vec"));
        assert_eq!(
            index.cross_crate_path(&CrossCrateKey {
                crate_name: "alloc".into(),
                path: vec!["vec".into(), "Vec".into()],
            }),
            Some("std::Vec")
        );
        assert!(
            index
                .by_key
                .get(&CrossCrateKey {
                    crate_name: "alloc".into(),
                    path: vec!["vec".into(), "Vec".into()],
                })
                .is_some_and(|paths| paths.contains(&"std::vec::Vec".into()))
        );
        assert_eq!(
            index.cross_crate_path(&CrossCrateKey {
                crate_name: "hashbrown".into(),
                path: vec!["map".into(), "HashMap".into()],
            }),
            Some("std::collections::HashMap")
        );
        assert!(!index.is_public(Id(6)));
        Ok(())
    }

    #[test]
    fn standard_index_expands_external_module_globs() -> Result<()> {
        let core = serde_json::json!({
            "root": 1,
            "index": {
                "1": {"crate_id": 0, "name": "core", "visibility": "public", "inner": {"module": {"items": [2]}}},
                "2": {"crate_id": 0, "name": "hash", "visibility": "public", "inner": {"module": {"items": [3]}}},
                "3": {"crate_id": 0, "name": "Hash", "visibility": "public", "inner": {"trait": {}}}
            },
            "paths": {
                "2": {"crate_id": 0, "path": ["core", "hash"], "kind": "module"},
                "3": {"crate_id": 0, "path": ["core", "hash", "Hash"], "kind": "trait"}
            },
            "external_crates": {}
        });
        let std = serde_json::json!({
            "root": 1,
            "index": {
                "1": {"crate_id": 0, "name": "std", "visibility": "public", "inner": {"module": {"items": [2]}}},
                "2": {"crate_id": 0, "name": "hash", "visibility": "public", "inner": {"module": {"items": [3]}}},
                "3": {"crate_id": 0, "name": "hash", "visibility": "public", "inner": {"use": {"source": "core::hash", "name": "hash", "id": 90, "is_glob": true}}}
            },
            "paths": {"90": {"crate_id": 1, "path": ["core", "hash"], "kind": "module"}},
            "external_crates": {"1": {"name": "core"}}
        });
        let core = PublicPathIndex::build_raw(&core, "core")?;
        let mut std = PublicPathIndex::build_raw(&std, "std")?;
        std.graft_module_reexports(&[&core]);
        assert_eq!(
            std.cross_crate_path(&CrossCrateKey {
                crate_name: "core".into(),
                path: vec!["hash".into(), "Hash".into()],
            }),
            Some("std::hash::Hash")
        );
        Ok(())
    }

    fn item(id: u32, name: &str, visibility: Visibility, inner: ItemEnum) -> Item {
        Item {
            id: Id(id),
            crate_id: 0,
            name: Some(name.into()),
            span: None,
            visibility,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            stability: None,
            const_stability: None,
            inner,
        }
    }

    fn module(items: &[u32], is_crate: bool) -> ItemEnum {
        ItemEnum::Module(Module {
            is_crate,
            items: items.iter().copied().map(Id).collect(),
            is_stripped: false,
        })
    }

    fn empty_crate(name: &str, root_items: &[u32]) -> Crate {
        let mut index = HashMap::new();
        index.insert(
            Id(0),
            item(0, name, Visibility::Public, module(root_items, true)),
        );
        Crate {
            root: Id(0),
            crate_version: None,
            includes_private: true,
            index,
            paths: HashMap::new(),
            external_crates: HashMap::new(),
            target: Target {
                triple: "x86_64-unknown-linux-gnu".into(),
                target_features: Vec::new(),
            },
            format_version: 0,
        }
    }

    fn summary(crate_id: u32, path: &[&str], kind: ItemKind) -> ItemSummary {
        ItemSummary {
            crate_id,
            path: path.iter().map(|segment| (*segment).to_string()).collect(),
            kind,
        }
    }

    fn path(id: u32, written: &str) -> Path {
        Path {
            id: Id(id),
            path: written.into(),
            args: None,
        }
    }

    fn add_alias(crate_data: &mut Crate, id: u32, name: &str, target: u32, glob: bool) {
        crate_data.index.insert(
            Id(id),
            item(
                id,
                name,
                Visibility::Public,
                ItemEnum::Use(Use {
                    source: "hidden::Item".into(),
                    name: name.into(),
                    id: Some(Id(target)),
                    is_glob: glob,
                }),
            ),
        );
    }

    #[test]
    fn public_paths_follow_reexports_and_choose_shortest_lexical_path() {
        let mut data = empty_crate("fixture", &[1, 2, 3]);
        data.index.insert(
            Id(1),
            item(1, "alpha", Visibility::Public, module(&[4], false)),
        );
        data.index.insert(
            Id(2),
            item(2, "beta", Visibility::Public, module(&[5], false)),
        );
        data.index.insert(
            Id(3),
            item(3, "hidden", Visibility::Default, module(&[6], false)),
        );
        data.index.insert(
            Id(7),
            item(7, "Thing", Visibility::Public, module(&[], false)),
        );
        add_alias(&mut data, 4, "Thing", 7, false);
        add_alias(&mut data, 5, "Thing", 7, false);
        add_alias(&mut data, 6, "Thing", 7, false);
        data.paths.insert(
            Id(7),
            summary(0, &["fixture", "hidden", "Thing"], ItemKind::Module),
        );

        let index = PublicPathIndex::build(&data);
        assert_eq!(index.canonical_path(Id(7)), Some("crate::alpha::Thing"));
        assert_eq!(
            index.public_paths(Id(7)).unwrap(),
            ["crate::alpha::Thing", "crate::beta::Thing"]
        );
        assert!(!index.is_public(Id(3)));

        add_alias(&mut data, 8, "PublicThing", 7, false);
        let ItemEnum::Module(root) = &mut data.index.get_mut(&Id(0)).unwrap().inner else {
            panic!("root module expected");
        };
        root.items.push(Id(8));
        let index = PublicPathIndex::build(&data);
        assert_eq!(index.canonical_path(Id(7)), Some("crate::PublicThing"));
    }

    #[test]
    fn names_use_ids_and_import_only_distinct_rendered_external_items() -> Result<()> {
        let mut data = empty_crate("fixture", &[1, 2]);
        data.index.insert(
            Id(1),
            item(1, "Result", Visibility::Public, module(&[], false)),
        );
        data.index.insert(
            Id(2),
            item(
                2,
                "Types",
                Visibility::Public,
                ItemEnum::TypeAlias(TypeAlias {
                    type_: Type::Tuple(vec![
                        Type::ResolvedPath(path(10, "io::Result")),
                        Type::ResolvedPath(path(11, "serde::Serialize")),
                        Type::ResolvedPath(path(12, "other::Serialize")),
                        Type::ResolvedPath(path(13, "renamed::Public")),
                    ]),
                    generics: Generics {
                        params: Vec::new(),
                        where_predicates: Vec::new(),
                    },
                }),
            ),
        );
        data.paths
            .insert(Id(1), summary(0, &["fixture", "Result"], ItemKind::Module));
        data.paths.insert(
            Id(10),
            summary(1, &["std", "io", "Result"], ItemKind::TypeAlias),
        );
        data.paths.insert(
            Id(11),
            summary(2, &["serde", "ser", "Serialize"], ItemKind::Trait),
        );
        data.paths
            .insert(Id(12), summary(3, &["other", "Serialize"], ItemKind::Trait));
        data.paths.insert(
            Id(13),
            summary(4, &["member", "private", "Public"], ItemKind::Struct),
        );
        for (id, name) in [(1, "std"), (2, "serde"), (3, "other"), (4, "renamed")] {
            data.external_crates.insert(
                id,
                ExternalCrate {
                    name: name.into(),
                    html_root_url: None,
                    path: PathBuf::new(),
                },
            );
        }
        let mut member_data = empty_crate("member", &[1]);
        member_data.index.insert(
            Id(1),
            item(1, "Public", Visibility::Public, module(&[], false)),
        );
        member_data.paths.insert(
            Id(1),
            summary(0, &["member", "private", "Public"], ItemKind::Struct),
        );
        let member = NamedCrate {
            package_name: "member-package".into(),
            crate_name: "member".into(),
            index: PublicPathIndex::build(&member_data),
        };
        let mut std_data = empty_crate("std", &[1]);
        std_data.index.insert(
            Id(1),
            item(1, "io", Visibility::Public, module(&[2], false)),
        );
        std_data.index.insert(
            Id(2),
            item(2, "Result", Visibility::Public, module(&[], false)),
        );
        std_data.paths.insert(
            Id(2),
            summary(0, &["std", "io", "Result"], ItemKind::TypeAlias),
        );
        let standard = NamedCrate {
            package_name: "std".into(),
            crate_name: "std".into(),
            index: PublicPathIndex::build(&std_data),
        };
        let aliases = BTreeMap::from([
            ("serde".into(), "serde".into()),
            ("other".into(), "other".into()),
            ("renamed".into(), "member-package".into()),
        ]);
        let local = PublicPathIndex::build(&data);
        let plan = NamePlan::new(&data, &local, &[member], &[standard], &aliases)?;
        let paths = [
            path(10, "io::Result"),
            path(11, "serde::Serialize"),
            path(12, "other::Serialize"),
            path(13, "renamed::Public"),
        ];
        let output =
            plan.render(|| Ok(paths.iter().map(render_path).collect::<Vec<_>>().join(" ")))?;
        assert_eq!(
            output,
            "std::io::Result serde::Serialize other::Serialize Public"
        );
        assert_eq!(plan.use_block(), "use renamed::Public;\n\n");
        assert_eq!(
            plan.exposed_packages(),
            ["member-package", "other", "serde"]
        );
        assert_eq!(plan.definition_fallbacks(), 0);
        Ok(())
    }

    #[test]
    fn local_collisions_and_unnameable_references_keep_full_paths() -> Result<()> {
        let mut data = empty_crate("fixture", &[1, 2, 3, 4]);
        data.index.insert(
            Id(1),
            item(1, "alpha", Visibility::Public, module(&[5], false)),
        );
        data.index.insert(
            Id(2),
            item(2, "beta", Visibility::Public, module(&[6], false)),
        );
        data.index.insert(
            Id(3),
            item(3, "hidden", Visibility::Default, module(&[7], false)),
        );
        data.index.insert(
            Id(5),
            item(5, "Entry", Visibility::Public, module(&[], false)),
        );
        data.index.insert(
            Id(6),
            item(6, "Entry", Visibility::Public, module(&[], false)),
        );
        data.index.insert(
            Id(7),
            item(7, "Private", Visibility::Public, module(&[], false)),
        );
        data.index.insert(
            Id(4),
            item(
                4,
                "Types",
                Visibility::Public,
                ItemEnum::TypeAlias(TypeAlias {
                    type_: Type::Tuple(vec![
                        Type::ResolvedPath(path(5, "alpha::Entry")),
                        Type::ResolvedPath(path(6, "beta::Entry")),
                        Type::ResolvedPath(path(7, "hidden::Private")),
                        Type::ResolvedPath(path(10, "other::Entry")),
                    ]),
                    generics: Generics {
                        params: Vec::new(),
                        where_predicates: Vec::new(),
                    },
                }),
            ),
        );
        for (id, path) in [
            (5, vec!["fixture", "alpha", "Entry"]),
            (6, vec!["fixture", "beta", "Entry"]),
            (7, vec!["fixture", "hidden", "Private"]),
        ] {
            data.paths
                .insert(Id(id), summary(0, &path, ItemKind::Module));
        }
        data.paths
            .insert(Id(10), summary(1, &["other", "Entry"], ItemKind::Struct));
        data.external_crates.insert(
            1,
            ExternalCrate {
                name: "other".into(),
                html_root_url: None,
                path: PathBuf::new(),
            },
        );
        let local = PublicPathIndex::build(&data);
        let aliases = BTreeMap::from([("other".into(), "other-package".into())]);
        let plan = NamePlan::new(&data, &local, &[], &[], &aliases)?;
        let paths = [
            path(5, "alpha::Entry"),
            path(6, "beta::Entry"),
            path(7, "hidden::Private"),
            path(10, "other::Entry"),
        ];
        let output =
            plan.render(|| Ok(paths.iter().map(render_path).collect::<Vec<_>>().join(" ")))?;
        assert_eq!(
            output,
            "crate::alpha::Entry crate::beta::Entry crate::hidden::Private other::Entry"
        );
        assert!(plan.use_block().is_empty());
        assert_eq!(plan.unnameable(), ["crate::hidden::Private"]);
        assert_eq!(plan.definition_fallbacks(), 1);
        Ok(())
    }

    #[test]
    fn a_facade_written_path_names_the_facade_package() -> Result<()> {
        let mut data = empty_crate("fixture", &[1]);
        data.index.insert(
            Id(1),
            item(
                1,
                "Position",
                Visibility::Public,
                ItemEnum::TypeAlias(TypeAlias {
                    type_: Type::Tuple(vec![
                        Type::ResolvedPath(path(10, "emath::Vec2")),
                        Type::ResolvedPath(path(10, "egui::Vec2")),
                    ]),
                    generics: Generics {
                        params: Vec::new(),
                        where_predicates: Vec::new(),
                    },
                }),
            ),
        );
        data.paths
            .insert(Id(10), summary(1, &["emath", "Vec2"], ItemKind::Struct));
        data.external_crates.insert(
            1,
            ExternalCrate {
                name: "emath".into(),
                html_root_url: None,
                path: PathBuf::new(),
            },
        );
        let aliases = BTreeMap::from([("egui".into(), "egui".into())]);
        let local = PublicPathIndex::build(&data);
        let plan = NamePlan::new(&data, &local, &[], &[], &aliases)?;
        let output = plan.render(|| Ok(render_path(&path(10, "emath::Vec2"))))?;
        assert_eq!(output, "Vec2");
        assert_eq!(plan.use_block(), "use egui::Vec2;\n\n");
        assert_eq!(plan.exposed_packages(), ["egui"]);
        assert_eq!(plan.definition_fallbacks(), 0);
        Ok(())
    }

    #[test]
    fn definition_fallback_still_reports_its_exposed_package() {
        let aliases = BTreeMap::new();
        let candidate =
            NameCandidate::external("serde_core::ser::Serialize".into(), true, &aliases);
        assert_eq!(candidate.exposed_package.as_deref(), Some("serde_core"));
        assert!(candidate.fallback);
        assert_eq!(
            NameCandidate::external("std::hash::Hash".into(), false, &aliases).exposed_package,
            None
        );
    }

    #[test]
    fn transitive_package_uses_cargo_artifact_or_docs_rs_name() {
        let mut data = empty_crate("fixture", &[]);
        data.external_crates.insert(
            1,
            ExternalCrate {
                name: "isolation_model".into(),
                html_root_url: None,
                path: PathBuf::from("/target/build/isolation-model/hash/out/lib.rmeta"),
            },
        );
        data.external_crates.insert(
            2,
            ExternalCrate {
                name: "serde_core".into(),
                html_root_url: Some("https://docs.rs/serde_core/1.0.0/".into()),
                path: PathBuf::new(),
            },
        );
        assert_eq!(
            external_package(&data, "isolation_model"),
            Some("isolation-model".into())
        );
        assert_eq!(
            external_package(&data, "serde_core"),
            Some("serde_core".into())
        );
    }
}
