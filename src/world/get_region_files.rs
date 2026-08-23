use crate::world::session_lock::{SessionLock, acquire_session_lock};
use crate::world::validate::validate_worlds;
use std::error::Error;
use std::path::{Path, PathBuf};

/// Region files of every world, deduplicated, plus the session-lock handles
/// that must stay alive while those files are processed.
#[derive(Debug)]
pub struct WorldRegions {
    pub region_files: Vec<PathBuf>,
    // Field order matters: locks are released (files closed) after the
    // region list is no longer used.
    _locks: Vec<SessionLock>,
}

pub fn get_region_files(world_paths: &[PathBuf]) -> Result<WorldRegions, Box<dyn Error>> {
    let worlds = validate_worlds(world_paths)?;

    // Deduplicate world roots first: the same directory listed twice must be
    // locked once (a second exclusive flock on an independent open would
    // conflict with our own first lock).
    let mut seen_worlds = std::collections::HashSet::new();
    let unique_worlds: Vec<&PathBuf> = worlds
        .iter()
        .filter(|w| seen_worlds.insert((*w).clone()))
        .collect();

    // Lock every world BEFORE listing any file: if a server is running on any
    // of them we abort before touching anything.
    let mut locks = Vec::with_capacity(unique_worlds.len());
    for world in &unique_worlds {
        locks.push(acquire_session_lock(world)?);
    }

    let mut seen = std::collections::HashSet::new();
    let region_files = worlds
        .iter()
        .flat_map(|world| get_region_files_from_world(world))
        .filter(|path| seen.insert(path.clone()))
        .collect::<Vec<_>>();

    Ok(WorldRegions {
        region_files,
        _locks: locks,
    })
}

fn get_region_files_from_world(world_dir: &Path) -> Vec<PathBuf> {
    let mut region_files = Vec::new();

    // Modern layout (since 26.1): dimensions/<namespace>/<dimension>/region
    // Covers the vanilla dimensions (minecraft:overworld, minecraft:the_nether,
    // minecraft:the_end) as well as any custom/modded dimensions.
    if let Ok(namespaces) = std::fs::read_dir(world_dir.join("dimensions")) {
        for namespace in namespaces.flatten() {
            if let Ok(dimensions) = std::fs::read_dir(namespace.path()) {
                for dimension in dimensions.flatten() {
                    region_files.extend(get_region_dir(dimension.path()));
                }
            }
        }
    }

    // Legacy layout (< 26.1): root `region`, `DIM-1/region`, `DIM1/region`.
    // Kept for worlds not yet upgraded by the game and for Bukkit-style server
    // layouts where each dimension is a separate world directory.
    region_files.extend(get_region_dir(world_dir.to_path_buf()));
    region_files.extend(get_region_dir(world_dir.join("DIM-1")));
    region_files.extend(get_region_dir(world_dir.join("DIM1")));

    region_files
}

fn get_region_dir(dimension_directory: PathBuf) -> Vec<PathBuf> {
    get_mca_files(dimension_directory.join("region"))
}

fn get_mca_files(region_directory: PathBuf) -> Vec<PathBuf> {
    std::fs::read_dir(region_directory)
        .map(|dir| {
            dir.flatten()
                .map(|entry| entry.path())
                // mcc files are not supported yet
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("mca"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mwt_regions_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn test_modern_and_legacy_layouts_are_both_discovered() {
        let tmp = unique_tmp_dir("layout");
        // Modern layout (>= 26.1): vanilla dimensions under dimensions/minecraft
        for dim in ["overworld", "the_nether", "the_end"] {
            let region_dir = tmp.join("dimensions/minecraft").join(dim).join("region");
            std::fs::create_dir_all(&region_dir).unwrap();
            std::fs::write(region_dir.join("r.0.0.mca"), b"x").unwrap();
        }
        // Custom namespaces/dimensions must be covered too
        let custom = tmp.join("dimensions/myplugin/custom_dim/region");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("r.0.0.mca"), b"x").unwrap();
        // Legacy layout (< 26.1)
        for dim in ["", "DIM-1", "DIM1"] {
            let region_dir = tmp.join(dim).join("region");
            std::fs::create_dir_all(&region_dir).unwrap();
            std::fs::write(region_dir.join("r.1.1.mca"), b"x").unwrap();
        }

        let files = get_region_files_from_world(&tmp);
        assert_eq!(files.len(), 7, "all modern + legacy regions must be found");
        assert!(files.iter().all(|p| p.extension().unwrap() == "mca"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_missing_dimension_dirs_are_tolerated() {
        let tmp = unique_tmp_dir("empty");
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(get_region_files_from_world(&tmp).is_empty());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Duplicate world roots must not yield duplicate region files: the same
    /// .mca processed twice concurrently would race rename/remove against
    /// itself.
    #[test]
    fn test_duplicate_world_roots_deduplicate_region_files() {
        let tmp = unique_tmp_dir("dedup");
        let region_dir = tmp.join("region");
        std::fs::create_dir_all(&region_dir).unwrap();
        std::fs::write(region_dir.join("r.0.0.mca"), b"x").unwrap();
        std::fs::write(tmp.join("level.dat"), b"fake").unwrap();
        let worlds = vec![tmp.clone(), tmp.clone()];
        let result = get_region_files(&worlds).expect("locks and listing must succeed");
        assert_eq!(
            result.region_files.len(),
            1,
            "duplicate roots must collapse to one file entry"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A held session.lock (simulating a running server) must abort the scan.
    #[test]
    fn test_held_session_lock_aborts() {
        let tmp = unique_tmp_dir("held");
        let region_dir = tmp.join("region");
        std::fs::create_dir_all(&region_dir).unwrap();
        std::fs::write(region_dir.join("r.0.0.mca"), b"x").unwrap();
        std::fs::write(tmp.join("level.dat"), b"fake").unwrap();

        let _server_lock = crate::world::session_lock::acquire_session_lock(&tmp)
            .expect("simulated server lock");

        let worlds = vec![tmp.clone()];
        let err = get_region_files(&worlds).expect_err("locked world must be refused");
        assert!(
            err.to_string().contains("session.lock"),
            "error must mention session.lock, got: {err}"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }
}
