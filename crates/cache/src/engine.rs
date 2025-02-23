use crate::helpers::{is_writable, LOG_TARGET};
use crate::items::{CacheItem, ProjectsState, RunTargetState, WorkspaceState};
use crate::runfiles::CacheRunfile;
use moon_config::constants::CONFIG_DIRNAME;
use moon_error::MoonError;
use moon_logger::{color, debug, trace};
use moon_utils::fs;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub struct CacheEngine {
    /// The `.moon/cache` directory relative to workspace root.
    /// Contains cached items pertaining to runs and processes.
    pub dir: PathBuf,

    /// The `.moon/cache/hashes` directory. Stores hash contents.
    pub hashes_dir: PathBuf,

    /// The `.moon/cache/runs` directory. Stores run states and runfiles.
    pub runs_dir: PathBuf,

    /// The `.moon/cache/out` directory. Stores task output.
    pub outputs_dir: PathBuf,
}

impl CacheEngine {
    pub async fn create(workspace_root: &Path) -> Result<Self, MoonError> {
        let dir = workspace_root.join(CONFIG_DIRNAME).join("cache");
        let hashes_dir = dir.join("hashes");
        let runs_dir = dir.join("runs");
        let outputs_dir = dir.join("out");

        debug!(
            target: LOG_TARGET,
            "Creating cache engine at {}",
            color::path(&dir)
        );

        fs::create_dir_all(&hashes_dir).await?;
        fs::create_dir_all(&runs_dir).await?;
        fs::create_dir_all(&outputs_dir).await?;

        Ok(CacheEngine {
            dir,
            hashes_dir,
            runs_dir,
            outputs_dir,
        })
    }

    pub async fn cache_run_target_state(
        &self,
        target_id: &str,
    ) -> Result<CacheItem<RunTargetState>, MoonError> {
        let path: PathBuf = [&target_id.replace(':', "/"), "lastRunState.json"]
            .iter()
            .collect();

        CacheItem::load(
            self.runs_dir.join(path),
            RunTargetState {
                target: String::from(target_id),
                ..RunTargetState::default()
            },
            0,
        )
        .await
    }

    pub async fn cache_projects_state(&self) -> Result<CacheItem<ProjectsState>, MoonError> {
        CacheItem::load(
            self.dir.join("projectsState.json"),
            ProjectsState::default(),
            90000, // Cache for 3 minutes
        )
        .await
    }

    pub async fn cache_workspace_state(&self) -> Result<CacheItem<WorkspaceState>, MoonError> {
        CacheItem::load(
            self.dir.join("workspaceState.json"),
            WorkspaceState::default(),
            0,
        )
        .await
    }

    pub async fn create_runfile<T: DeserializeOwned + Serialize>(
        &self,
        project_id: &str,
        data: &T,
    ) -> Result<CacheRunfile, MoonError> {
        let path: PathBuf = [project_id, "runfile.json"].iter().collect();

        CacheRunfile::load(self.runs_dir.join(path), data).await
    }

    pub async fn delete_hash(&self, hash: &str) -> Result<(), MoonError> {
        if is_writable() {
            let path = self.hashes_dir.join(format!("{}.json", hash));

            trace!(target: "moon:cache:hash", "Deleting hash {}", color::path(&path));

            // Remove the hash file itself
            fs::remove_file(&path).await?;

            // And the output with the hash
            fs::remove_dir_all(&self.outputs_dir.join(hash)).await?;
        }

        Ok(())
    }

    pub async fn delete_runfiles(&self) -> Result<(), MoonError> {
        let entries = fs::read_dir(&self.runs_dir).await?;

        for entry in entries {
            let path = entry.path();

            if path.is_dir() {
                let runfile = path.join("runfile.json");

                trace!(target: "moon:cache:runfile", "Deleting runfile {}", color::path(&runfile));

                fs::remove_file(&runfile).await?;
            }
        }

        Ok(())
    }

    pub async fn link_task_output_to_out(
        &self,
        hash: &str,
        source_root: &Path,
        source_path: &Path,
    ) -> Result<(), MoonError> {
        if is_writable() {
            let dest_root = self.outputs_dir.join(hash);

            fs::create_dir_all(&dest_root).await?;

            trace!(
                target: LOG_TARGET,
                "Hardlinking output {} to {}",
                color::path(source_path),
                color::path(&dest_root)
            );

            if source_path.is_file() {
                fs::link_file(source_root, source_path, &dest_root).await?;
            } else {
                fs::link_dir(source_root, source_path, &dest_root).await?;
            }
        }

        Ok(())
    }

    pub async fn save_hash<T>(&self, hash: &str, hasher: &T) -> Result<(), MoonError>
    where
        T: ?Sized + Serialize,
    {
        if is_writable() {
            let path = self.hashes_dir.join(format!("{}.json", hash));

            trace!(target: "moon:cache:hash", "Writing hash {}", color::path(&path));

            fs::write_json(&path, &hasher, true).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::{run_with_env, to_millis};
    use assert_fs::prelude::*;
    use serial_test::serial;
    use std::fs;

    mod create {
        use super::*;

        #[tokio::test]
        #[serial]
        async fn creates_dirs() {
            let dir = assert_fs::TempDir::new().unwrap();

            CacheEngine::create(dir.path()).await.unwrap();

            assert!(dir.path().join(".moon/cache").exists());
            assert!(dir.path().join(".moon/cache/hashes").exists());
            assert!(dir.path().join(".moon/cache/runs").exists());
            assert!(dir.path().join(".moon/cache/out").exists());

            dir.close().unwrap();
        }
    }

    mod delete_runfiles {
        use super::*;

        #[tokio::test]
        #[serial]
        async fn deletes_dir() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();

            let runfile1 = cache
                .create_runfile("123", &"content".to_owned())
                .await
                .unwrap();
            let runfile2 = cache
                .create_runfile("456", &"content".to_owned())
                .await
                .unwrap();

            assert!(runfile1.path.exists());
            assert!(runfile2.path.exists());

            cache.delete_runfiles().await.unwrap();

            assert!(!runfile1.path.exists());
            assert!(!runfile2.path.exists());

            dir.close().unwrap();
        }
    }

    mod delete_hash {
        use super::*;

        #[tokio::test]
        #[serial]
        async fn deletes_files() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();

            dir.child(".moon/cache/hashes/abc123.json")
                .write_str("{}")
                .unwrap();

            dir.child(".moon/cache/out/abc123/file.js")
                .write_str("")
                .unwrap();

            let hash_file = cache.hashes_dir.join("abc123.json");
            let out_file = cache.outputs_dir.join("abc123/file.js");

            assert!(hash_file.exists());
            assert!(out_file.exists());

            cache.delete_hash("abc123").await.unwrap();

            assert!(!hash_file.exists());
            assert!(!out_file.exists());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_delete_if_cache_off() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();

            dir.child(".moon/cache/hashes/abc123.json")
                .write_str("{}")
                .unwrap();

            dir.child(".moon/cache/out/abc123/file.js")
                .write_str("")
                .unwrap();

            let hash_file = cache.hashes_dir.join("abc123.json");
            let out_file = cache.outputs_dir.join("abc123/file.js");

            assert!(hash_file.exists());
            assert!(out_file.exists());

            run_with_env("off", || cache.delete_hash("abc123"))
                .await
                .unwrap();

            assert!(hash_file.exists());
            assert!(out_file.exists());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_delete_if_cache_readonly() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();

            dir.child(".moon/cache/hashes/abc123.json")
                .write_str("{}")
                .unwrap();

            dir.child(".moon/cache/out/abc123/file.js")
                .write_str("")
                .unwrap();

            let hash_file = cache.hashes_dir.join("abc123.json");
            let out_file = cache.outputs_dir.join("abc123/file.js");

            assert!(hash_file.exists());
            assert!(out_file.exists());

            run_with_env("read", || cache.delete_hash("abc123"))
                .await
                .unwrap();

            assert!(hash_file.exists());
            assert!(out_file.exists());

            dir.close().unwrap();
        }
    }

    mod create_runfile {
        use super::*;

        #[tokio::test]
        #[serial]
        async fn creates_runfile_on_call() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let runfile = cache
                .create_runfile("123", &"content".to_owned())
                .await
                .unwrap();

            assert!(runfile.path.exists());

            assert_eq!(
                fs::read_to_string(dir.path().join(".moon/cache/runs/123/runfile.json")).unwrap(),
                "\"content\""
            );

            dir.close().unwrap();
        }
    }

    mod cache_run_target_state {
        use super::*;

        #[tokio::test]
        #[serial]
        async fn creates_parent_dir_on_call() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = cache.cache_run_target_state("foo:bar").await.unwrap();

            assert!(!item.path.exists());
            assert!(item.path.parent().unwrap().exists());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn loads_cache_if_it_exists() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/runs/foo/bar/lastRunState.json")
                .write_str(r#"{"exitCode":123,"hash":"","lastRunTime":0,"stderr":"","stdout":"","target":"foo:bar"}"#)
                .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = cache.cache_run_target_state("foo:bar").await.unwrap();

            assert_eq!(
                item.item,
                RunTargetState {
                    exit_code: 123,
                    target: String::from("foo:bar"),
                    ..RunTargetState::default()
                }
            );

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn loads_cache_if_it_exists_and_cache_is_readonly() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/runs/foo/bar/lastRunState.json")
                .write_str(r#"{"exitCode":123,"hash":"","lastRunTime":0,"stderr":"","stdout":"","target":"foo:bar"}"#)
                .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = run_with_env("read", || cache.cache_run_target_state("foo:bar"))
                .await
                .unwrap();

            assert_eq!(
                item.item,
                RunTargetState {
                    exit_code: 123,
                    target: String::from("foo:bar"),
                    ..RunTargetState::default()
                }
            );

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_load_if_it_exists_but_cache_is_off() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/runs/foo/bar/lastRunState.json")
                .write_str(r#"{"exitCode":123,"hash":"","lastRunTime":0,"stderr":"","stdout":"","target":"foo:bar"}"#)
                .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = run_with_env("off", || cache.cache_run_target_state("foo:bar"))
                .await
                .unwrap();

            assert_eq!(
                item.item,
                RunTargetState {
                    target: String::from("foo:bar"),
                    ..RunTargetState::default()
                }
            );

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn saves_to_cache() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let mut item = cache.cache_run_target_state("foo:bar").await.unwrap();

            item.item.exit_code = 123;

            run_with_env("", || item.save()).await.unwrap();

            assert_eq!(
                fs::read_to_string(item.path).unwrap(),
                r#"{"exitCode":123,"hash":"","lastRunTime":0,"stderr":"","stdout":"","target":"foo:bar"}"#
            );

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_save_if_cache_off() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let mut item = cache.cache_run_target_state("foo:bar").await.unwrap();

            item.item.exit_code = 123;

            run_with_env("off", || item.save()).await.unwrap();

            assert!(!item.path.exists());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_save_if_cache_readonly() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let mut item = cache.cache_run_target_state("foo:bar").await.unwrap();

            item.item.exit_code = 123;

            run_with_env("read", || item.save()).await.unwrap();

            assert!(!item.path.exists());

            dir.close().unwrap();
        }
    }

    mod cache_workspace_state {
        use super::*;

        #[tokio::test]
        #[serial]
        async fn creates_parent_dir_on_call() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = cache.cache_workspace_state().await.unwrap();

            assert!(!item.path.exists());
            assert!(item.path.parent().unwrap().exists());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn loads_cache_if_it_exists() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/workspaceState.json")
                .write_str(r#"{"lastNodeInstallTime":123}"#)
                .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = cache.cache_workspace_state().await.unwrap();

            assert_eq!(
                item.item,
                WorkspaceState {
                    last_node_install_time: 123,
                    last_version_check_time: 0,
                }
            );

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn loads_cache_if_it_exists_and_cache_is_readonly() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/workspaceState.json")
                .write_str(r#"{"lastNodeInstallTime":123}"#)
                .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = run_with_env("read", || cache.cache_workspace_state())
                .await
                .unwrap();

            assert_eq!(
                item.item,
                WorkspaceState {
                    last_node_install_time: 123,
                    last_version_check_time: 0,
                }
            );

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_load_if_it_exists_but_cache_is_off() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/workspaceState.json")
                .write_str(r#"{"lastNodeInstallTime":123}"#)
                .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = run_with_env("off", || cache.cache_workspace_state())
                .await
                .unwrap();

            assert_eq!(item.item, WorkspaceState::default());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn saves_to_cache() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let mut item = cache.cache_workspace_state().await.unwrap();

            item.item.last_node_install_time = 123;

            run_with_env("", || item.save()).await.unwrap();

            assert_eq!(
                fs::read_to_string(item.path).unwrap(),
                r#"{"lastNodeInstallTime":123,"lastVersionCheckTime":0}"#
            );

            dir.close().unwrap();
        }
    }

    mod cache_projects_state {
        use super::*;
        use filetime::{set_file_mtime, FileTime};
        use moon_utils::string_vec;
        use std::collections::HashMap;
        use std::time::SystemTime;

        #[tokio::test]
        #[serial]
        async fn creates_parent_dir_on_call() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = cache.cache_projects_state().await.unwrap();

            assert!(!item.path.exists());
            assert!(item.path.parent().unwrap().exists());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn loads_cache_if_it_exists() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/projectsState.json")
                .write_str(r#"{"globs":["**/*"],"projects":{"foo":"bar"}}"#)
                .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = cache.cache_projects_state().await.unwrap();

            assert_eq!(
                item.item,
                ProjectsState {
                    globs: string_vec!["**/*"],
                    projects: HashMap::from([("foo".to_owned(), "bar".to_owned())]),
                }
            );

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn loads_cache_if_it_exists_and_cache_is_readonly() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/projectsState.json")
                .write_str(r#"{"globs":["**/*"],"projects":{"foo":"bar"}}"#)
                .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = run_with_env("read", || cache.cache_projects_state())
                .await
                .unwrap();

            assert_eq!(
                item.item,
                ProjectsState {
                    globs: string_vec!["**/*"],
                    projects: HashMap::from([("foo".to_owned(), "bar".to_owned())]),
                }
            );

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_load_if_it_exists_but_cache_is_off() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/projectsState.json")
                .write_str(r#"{"globs":[],"projects":{"foo":"bar"}}"#)
                .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = run_with_env("off", || cache.cache_projects_state())
                .await
                .unwrap();

            assert_eq!(item.item, ProjectsState::default());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_load_if_it_exists_but_cache_is_stale() {
            let dir = assert_fs::TempDir::new().unwrap();

            dir.child(".moon/cache/projectsState.json")
                .write_str(r#"{"globs":[],"projects":{"foo":"bar"}}"#)
                .unwrap();

            let now = to_millis(SystemTime::now()) - 100000;

            set_file_mtime(
                dir.path().join(".moon/cache/projectsState.json"),
                FileTime::from_unix_time((now / 1000) as i64, 0),
            )
            .unwrap();

            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let item = cache.cache_projects_state().await.unwrap();

            assert_eq!(item.item, ProjectsState::default());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn saves_to_cache() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let mut item = cache.cache_projects_state().await.unwrap();

            item.item
                .projects
                .insert("foo".to_owned(), "bar".to_owned());

            run_with_env("", || item.save()).await.unwrap();

            assert_eq!(
                fs::read_to_string(item.path).unwrap(),
                r#"{"globs":[],"projects":{"foo":"bar"}}"#
            );

            dir.close().unwrap();
        }
    }

    mod save_hash {
        use super::*;
        use serde::Deserialize;

        #[derive(Default, Deserialize, Serialize)]
        struct TestHasher {
            field: String,
        }

        #[tokio::test]
        #[serial]
        async fn creates_hash_file() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let hasher = TestHasher::default();

            cache.save_hash("abc123", &hasher).await.unwrap();

            assert!(cache.hashes_dir.join("abc123.json").exists());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_create_if_cache_off() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let hasher = TestHasher::default();

            run_with_env("off", || cache.save_hash("abc123", &hasher))
                .await
                .unwrap();

            assert!(!cache.hashes_dir.join("abc123.json").exists());

            dir.close().unwrap();
        }

        #[tokio::test]
        #[serial]
        async fn doesnt_create_if_cache_readonly() {
            let dir = assert_fs::TempDir::new().unwrap();
            let cache = CacheEngine::create(dir.path()).await.unwrap();
            let hasher = TestHasher::default();

            run_with_env("read", || cache.save_hash("abc123", &hasher))
                .await
                .unwrap();

            assert!(!cache.hashes_dir.join("abc123.json").exists());

            dir.close().unwrap();
        }
    }
}
