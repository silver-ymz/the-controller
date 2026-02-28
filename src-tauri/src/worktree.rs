use git2::Repository;
use std::path::{Path, PathBuf};

pub struct WorktreeManager;

impl WorktreeManager {
    /// Create a new git worktree for the given branch name under `{repo_path}/.worktrees/{branch_name}`.
    ///
    /// Opens the repository, creates a new branch from HEAD, and sets up
    /// the worktree directory. Returns the path to the worktree directory.
    pub fn create_worktree(repo_path: &str, branch_name: &str) -> Result<PathBuf, String> {
        let repo = Repository::open(repo_path).map_err(|e| format!("failed to open repo: {}", e))?;

        let worktrees_dir = Path::new(repo_path).join(".worktrees");
        let worktree_dir = worktrees_dir.join(branch_name);

        if worktree_dir.exists() {
            return Err(format!(
                "worktree directory already exists: {}",
                worktree_dir.display()
            ));
        }

        // Create the .worktrees/ parent directory
        std::fs::create_dir_all(&worktrees_dir)
            .map_err(|e| format!("failed to create .worktrees dir: {}", e))?;

        // Get HEAD commit and create a new branch from it
        let head = repo
            .head()
            .map_err(|e| format!("failed to get HEAD: {}", e))?;
        let commit = head
            .peel_to_commit()
            .map_err(|e| format!("failed to peel HEAD to commit: {}", e))?;
        let branch = repo
            .branch(branch_name, &commit, false)
            .map_err(|e| format!("failed to create branch '{}': {}", branch_name, e))?;

        // Create the worktree with the new branch as its HEAD
        let reference = branch.into_reference();
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(&reference));

        repo.worktree(branch_name, &worktree_dir, Some(&opts))
            .map_err(|e| format!("failed to create worktree: {}", e))?;

        Ok(worktree_dir)
    }

    /// Remove a worktree by deleting its directory and pruning the worktree reference.
    pub fn remove_worktree(repo_path: &str, branch_name: &str) -> Result<(), String> {
        let worktree_dir = Path::new(repo_path)
            .join(".worktrees")
            .join(branch_name);

        // Remove the worktree directory if it exists
        if worktree_dir.exists() {
            std::fs::remove_dir_all(&worktree_dir)
                .map_err(|e| format!("failed to remove worktree dir: {}", e))?;
        }

        // Prune the worktree reference
        let repo = Repository::open(repo_path)
            .map_err(|e| format!("failed to open repo: {}", e))?;

        if let Ok(wt) = repo.find_worktree(branch_name) {
            let mut prune_opts = git2::WorktreePruneOptions::new();
            prune_opts.valid(true);
            prune_opts.working_tree(true);
            wt.prune(Some(&mut prune_opts))
                .map_err(|e| format!("failed to prune worktree: {}", e))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a temporary git repo with an initial commit so HEAD exists.
    fn setup_test_repo() -> (TempDir, String) {
        let tmp = TempDir::new().expect("create temp dir");
        let repo_path = tmp.path().to_str().unwrap().to_string();

        let repo = Repository::init(&repo_path).expect("init repo");
        let sig = repo.signature().unwrap_or_else(|_| {
            git2::Signature::now("Test", "test@example.com").unwrap()
        });

        // Write an empty tree
        let tree_id = repo.treebuilder(None).unwrap().write().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();

        // Create initial commit
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .expect("initial commit");

        (tmp, repo_path)
    }

    #[test]
    fn test_create_and_remove_worktree() {
        let (_tmp, repo_path) = setup_test_repo();

        // Create a worktree
        let wt_path = WorktreeManager::create_worktree(&repo_path, "feature-test")
            .expect("create worktree");

        // Verify the worktree directory exists and has a .git marker
        assert!(wt_path.exists(), "worktree directory should exist");
        assert!(
            wt_path.join(".git").exists(),
            "worktree should have a .git file"
        );

        // Remove the worktree
        WorktreeManager::remove_worktree(&repo_path, "feature-test")
            .expect("remove worktree");

        // Verify the directory is gone
        assert!(!wt_path.exists(), "worktree directory should be removed");
    }

    #[test]
    fn test_duplicate_worktree_fails() {
        let (_tmp, repo_path) = setup_test_repo();

        // Create first worktree
        WorktreeManager::create_worktree(&repo_path, "dupe-branch")
            .expect("first create should succeed");

        // Try to create another with the same name - should fail
        let result = WorktreeManager::create_worktree(&repo_path, "dupe-branch");
        assert!(result.is_err(), "duplicate worktree should fail");
        assert!(
            result.unwrap_err().contains("already exists"),
            "error should mention 'already exists'"
        );
    }
}
