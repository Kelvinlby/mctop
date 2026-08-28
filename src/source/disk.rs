//! World size on disk.
//!
//! Walking a world folder is slow — a busy overworld is hundreds of thousands
//! of files — so this runs on a blocking thread on its own long interval, and
//! the interface keeps showing the previous figure while a scan is in flight.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use crate::config::WorldConfig;
use crate::metrics::{DiskUsage, WorldUsage};

/// Measure every configured world. Blocking; call from a blocking context.
pub fn scan(worlds: &[WorldConfig]) -> DiskUsage {
    let measured: Vec<WorldUsage> = worlds.iter().map(measure).collect();

    let free = measured
        .iter()
        .find(|world| world.path.exists())
        .and_then(|world| free_space(&world.path));

    DiskUsage {
        worlds: measured,
        free,
        scanned_at: Some(SystemTime::now()),
        scanning: false,
    }
}

fn measure(world: &WorldConfig) -> WorldUsage {
    let name = world
        .name
        .clone()
        .or_else(|| {
            world
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| world.path.display().to_string());

    let (bytes, files, partial) = directory_size(&world.path);

    // Folia and Paper split a dimension's chunk data across these folders, and
    // knowing which one has grown is what tells an operator where to look:
    // runaway `entities/` is a mob problem, runaway `region/` is a map problem.
    let subdirectory = |leaf: &str| directory_size(&world.path.join(leaf)).0;

    WorldUsage {
        name,
        path: world.path.clone(),
        bytes,
        files,
        region_bytes: subdirectory("region"),
        entity_bytes: subdirectory("entities"),
        poi_bytes: subdirectory("poi"),
        partial,
    }
}

/// Total bytes, file count, and whether anything was skipped.
///
/// Symlinked directories are not followed, so a world linked into itself, or
/// into the rest of the filesystem, cannot send the walk into a loop.
fn directory_size(root: &Path) -> (u64, u64, bool) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut partial = false;
    let mut stack = vec![root.to_path_buf()];

    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                // A world that is not there at all is reported as empty, not
                // as a partial read; anything else is genuinely skipped.
                partial |= directory != root || root.exists();
                continue;
            }
        };

        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                partial = true;
                continue;
            };

            if file_type.is_symlink() {
                continue;
            }

            if file_type.is_dir() {
                stack.push(entry.path());
            } else if let Ok(metadata) = entry.metadata() {
                bytes += metadata.len();
                files += 1;
            } else {
                partial = true;
            }
        }
    }

    (bytes, files, partial)
}

/// Free and total bytes of the filesystem holding `path`.
fn free_space(path: &Path) -> Option<(u64, u64)> {
    use sysinfo::Disks;

    let disks = Disks::new_with_refreshed_list();
    let canonical = path.canonicalize().ok()?;

    // Several mount points can be prefixes of the same path; the longest one is
    // the filesystem the world actually sits on.
    disks
        .list()
        .iter()
        .filter(|disk| canonical.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| (disk.available_space(), disk.total_space()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("mctop-test-{name}"));
        fs::remove_dir_all(&path).ok();
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn sums_a_tree() {
        let root = temp_dir("disk-sums");
        fs::write(root.join("level.dat"), b"12345").unwrap();
        fs::create_dir_all(root.join("region")).unwrap();
        fs::write(root.join("region/r.0.0.mca"), vec![0u8; 100]).unwrap();

        let (bytes, files, partial) = directory_size(&root);
        assert_eq!(bytes, 105);
        assert_eq!(files, 2);
        assert!(!partial);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_world_measures_as_empty() {
        let (bytes, files, partial) = directory_size(Path::new("/mctop/definitely/not/here"));
        assert_eq!((bytes, files), (0, 0));
        assert!(!partial);
    }

    #[test]
    fn breaks_a_world_down_by_subdirectory() {
        let root = temp_dir("disk-breakdown");
        for (leaf, size) in [("region", 300usize), ("entities", 200), ("poi", 100)] {
            fs::create_dir_all(root.join(leaf)).unwrap();
            fs::write(root.join(leaf).join("data"), vec![0u8; size]).unwrap();
        }
        fs::write(root.join("level.dat"), vec![0u8; 50]).unwrap();

        let usage = measure(&WorldConfig {
            name: None,
            path: root.clone(),
        });
        assert_eq!(usage.name, root.file_name().unwrap().to_string_lossy());
        assert_eq!(usage.bytes, 650);
        assert_eq!(usage.region_bytes, 300);
        assert_eq!(usage.entity_bytes, 200);
        assert_eq!(usage.poi_bytes, 100);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn totals_across_worlds() {
        let root = temp_dir("disk-total");
        let first = root.join("world");
        let second = root.join("world_nether");
        for path in [&first, &second] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("level.dat"), vec![0u8; 10]).unwrap();
        }

        let usage = scan(&[
            WorldConfig {
                name: Some("overworld".into()),
                path: first,
            },
            WorldConfig {
                name: None,
                path: second,
            },
        ]);
        assert_eq!(usage.total(), 20);
        assert_eq!(usage.worlds[0].name, "overworld");
        assert_eq!(usage.worlds[1].name, "world_nether");
        assert!(!usage.scanning);

        fs::remove_dir_all(&root).ok();
    }
}
