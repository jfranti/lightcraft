//! Imported profiles: `.cube` 3D LUTs used as creative profiles (Amount 0–200 % like the
//! built-in ones). A library on disk keeps a copy of each file in its `Profiles/` folder; the
//! list lives in the library's prefs and the LUTs are registered with the pipeline when the
//! library opens.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session};

/// One imported LUT profile.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LutProfile {
    /// `lut:<slug>`.
    pub id: String,
    pub name: String,
    pub group: String,
    /// The `.cube` file (the library's copy when there is a library on disk).
    pub file: String,
}

fn profiles_dir(s: &Session) -> Option<PathBuf> {
    s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.join("Profiles"))
}

/// Register every imported profile's LUT with the pipeline (missing files are skipped).
pub fn register_all(s: &Session) {
    for p in &s.lut_profiles {
        if let Some(l) = std::fs::read_to_string(&p.file).ok().and_then(|t| lightcraft_pipeline::lut::Lut3d::parse_cube(&t).ok()) {
            lightcraft_pipeline::lut::register(&p.id, l);
        }
    }
}

/// `.cube` files in `paths` (folders searched, zip bundles opened), and the files that couldn't
/// be read with the reason (`[name, reason]`).
fn cube_files(paths: &[String]) -> (Vec<(String, String, Option<String>)>, Vec<Value>) {
    // (name, text, group)
    let mut out = Vec::new();
    let mut failed = Vec::new();
    let stem = |f: &Path| f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            let cube = |f: &Path| f.extension().is_some_and(|x| x.eq_ignore_ascii_case("cube"));
            for ep in crate::walk::files_in(path, None, crate::walk::Limits::default(), cube).files {
                match std::fs::read_to_string(&ep) {
                    Ok(t) => {
                        let group = ep.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string());
                        out.push((stem(&ep), t, group));
                    }
                    Err(e) => failed.push(json!([stem(&ep), e.to_string()])),
                }
            }
        } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
            let b = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    failed.push(json!([stem(path), e.to_string()]));
                    continue;
                }
            };
            for (inner, data) in crate::preset_import::read_zip(&b).unwrap_or_default() {
                if inner.to_ascii_lowercase().ends_with(".cube") {
                    let ip = Path::new(&inner);
                    let group = ip
                        .parent()
                        .and_then(|d| d.file_name())
                        .map(|n| n.to_string_lossy().to_string())
                        .or_else(|| path.file_stem().map(|n| n.to_string_lossy().to_string()));
                    out.push((
                        ip.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        String::from_utf8_lossy(&data).to_string(),
                        group,
                    ));
                }
            }
        } else {
            match std::fs::read_to_string(path) {
                Ok(t) => out.push((stem(path), t, None)),
                Err(e) => failed.push(json!([stem(path), e.to_string()])),
            }
        }
    }
    (out, failed)
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.import";
    let paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if paths.is_empty() {
        return Err(bad(C, "no paths"));
    }
    let dir = profiles_dir(s);
    if let Some(d) = &dir {
        std::fs::create_dir_all(d).map_err(|e| bad(C, format!("{}: {e}", d.display())))?;
    }
    let (files, mut failed) = cube_files(&paths);
    let mut imported = Vec::new();
    for (name, text, group) in files {
        let lut = match lightcraft_pipeline::lut::Lut3d::parse_cube(&text) {
            Ok(l) => l,
            Err(e) => {
                failed.push(json!([name, e]));
                continue;
            }
        };
        let display = lut.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| name.clone());
        let group = super::super::preset_import::group_from_dir(&group.unwrap_or_default()).unwrap_or_else(|| "Imported".into());
        let slug = crate::presets::slug(&format!("{group}-{display}"));
        let mut id = format!("lut:{slug}");
        let mut n = 2;
        while s.lut_profiles.iter().any(|x| x.id == id) {
            id = format!("lut:{slug}-{n}");
            n += 1;
        }
        let file = match &dir {
            Some(d) => {
                let f = d.join(format!("{}.cube", id.trim_start_matches("lut:")));
                std::fs::write(&f, &text).map_err(|e| bad(C, format!("{}: {e}", f.display())))?;
                f.to_string_lossy().to_string()
            }
            None => {
                // no library on disk: remember the original (the in-memory session ends with the app)
                let f = std::env::temp_dir().join(format!("lightcraft-{}.cube", id.trim_start_matches("lut:")));
                std::fs::write(&f, &text).map_err(|e| bad(C, e.to_string()))?;
                f.to_string_lossy().to_string()
            }
        };
        lightcraft_pipeline::lut::register(&id, lut);
        s.lut_profiles.push(LutProfile { id: id.clone(), name: display.clone(), group: group.clone(), file });
        imported.push(json!({"id": id, "name": display, "group": group}));
    }
    s.save_prefs()?;
    Ok(json!({"imported": imported, "failed": failed}))
}

/// `profile.generate`: a seeded film look (`lightcraft_pipeline::film_seed`) added as a profile in the
/// "Seeded Film" group, the same way an imported `.cube` is; `path` also saves the `.cube` there.
fn generate(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "profile.generate";
    use lightcraft_pipeline::film_seed::FilmParams;
    let seed = match p.get("seed") {
        Some(v) => v.as_u64().or_else(|| v.as_str().and_then(|t| t.trim().parse().ok())).ok_or_else(|| bad(C, "seed must be a whole number"))?,
        // no seed: a fresh one from the clock, kept short enough to read out and share
        None => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64 % 1_000_000),
    };
    let size = p.get("size").and_then(Value::as_u64).unwrap_or(33).clamp(9, 65) as usize;
    let look = FilmParams::from_seed(seed);
    let cube = look.lut(size).to_cube();
    if let Some(path) = str_param(p, "path") {
        std::fs::write(path, &cube).map_err(|e| bad(C, format!("{path}: {e}")))?;
    }
    // through the import path (the library keeps its own copy), from a folder named for the group
    let root = std::env::temp_dir().join(format!("lightcraft-seeded-{}-{seed}", std::process::id()));
    let dir = root.join("Seeded Film");
    std::fs::create_dir_all(&dir).map_err(|e| bad(C, e.to_string()))?;
    std::fs::write(dir.join(format!("film-{seed}.cube")), &cube).map_err(|e| bad(C, e.to_string()))?;
    let r = import(s, &json!({"paths": [dir.to_string_lossy()]}));
    let _ = std::fs::remove_dir_all(&root);
    let mut r = r?;
    r["seed"] = json!(seed);
    r["archetype"] = json!(look.archetype.name());
    r["look"] = serde_json::to_value(&look).unwrap_or_default();
    Ok(r)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "profile.generate",
            "Generate Film Look",
            [],
            None,
            "{seed?: number (a fresh one when omitted), size?: 9..65 (33), path?: also save the .cube there} — a seeded film look (print, slide, negative, faded, cross-process or bleach bypass) as a creative profile in the Seeded Film group; the same seed always gives the same look → {imported: [{id, name, group}], seed, archetype, look}",
            always,
            generate
        ),
        cmd!(
            "profile.import",
            "Import Profiles",
            [],
            None,
            "{paths: [.cube file, folder or .zip]} — 3D LUTs become creative profiles (Amount 0–200 %; rendered on the CPU) in the profile browser's groups (folder / zip name, else Imported) → {imported: [{id, name, group}], failed}",
            always,
            import
        ),
        cmd!("profile.deleteImported", "Delete Imported Profile", [], None, "{id: lut:…}", always, |s, p| {
            let id = str_param(p, "id").ok_or_else(|| bad("profile.deleteImported", "missing id"))?.to_string();
            let Some(i) = s.lut_profiles.iter().position(|x| x.id == id) else {
                return Err(bad("profile.deleteImported", format!("no imported profile `{id}`")));
            };
            let gone = s.lut_profiles.remove(i);
            if profiles_dir(s).is_some_and(|d| Path::new(&gone.file).starts_with(d)) {
                let _ = std::fs::remove_file(&gone.file);
            }
            lightcraft_pipeline::lut::unregister(&id);
            s.save_prefs()?;
            Ok(json!({"deleted": id}))
        }),
    ]
}
